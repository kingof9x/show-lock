//! Listens to the fingerprint reader directly (Windows Biometric Framework),
//! so touching the sensor unlocks without any Windows Hello pop-up.
//!
//! Only a finger enrolled for the signed-in Windows user counts. Windows only
//! hands readings to the app in the foreground, which the lock screen is.
//! On PCs where the reader is reserved for Windows sign-in (Enhanced Sign-in
//! Security) or there is no reader, this reports `Unavailable` and the lock
//! screen offers the Windows Hello prompt instead.

use std::sync::mpsc::{self, Receiver};

use windows_sys::core::{GUID, HRESULT};
use windows_sys::Win32::Devices::BiometricFramework::{
    WinBioCloseSession, WinBioIdentify, WinBioOpenSession, WINBIO_E_BAD_CAPTURE, WINBIO_E_NO_MATCH,
    WINBIO_IDENTITY, WINBIO_POOL_SYSTEM,
};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Security::{
    GetLengthSid, GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use crate::lockscreen::FingerEvent;

const WINBIO_TYPE_FINGERPRINT: u32 = 0x0000_0008;
const WINBIO_FLAG_DEFAULT: u32 = 0;
const WINBIO_ID_TYPE_SID: u32 = 3;
/// `WINBIO_DB_DEFAULT` is the pointer value 1 in the Windows headers.
const WINBIO_DB_DEFAULT: *const GUID = std::ptr::without_provenance(1);

/// Start listening; events arrive on the receiver until a match or until the
/// reader turns out to be unusable.
pub fn listen() -> Receiver<FingerEvent> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("fingerprint".into())
        .spawn(move || {
            let Some(me) = current_user_sid() else {
                let _ = tx.send(FingerEvent::Unavailable);
                return;
            };
            let mut session = 0u32;
            let hr = unsafe {
                WinBioOpenSession(
                    WINBIO_TYPE_FINGERPRINT,
                    WINBIO_POOL_SYSTEM,
                    WINBIO_FLAG_DEFAULT,
                    std::ptr::null(),
                    0,
                    WINBIO_DB_DEFAULT,
                    &mut session,
                )
            };
            if hr < 0 {
                let _ = tx.send(FingerEvent::Unavailable);
                return;
            }
            loop {
                let mut unit = 0u32;
                let mut identity: WINBIO_IDENTITY = WINBIO_IDENTITY::default();
                let mut subfactor = 0u8;
                let mut reject = 0u32;
                let hr: HRESULT = unsafe {
                    WinBioIdentify(
                        session,
                        &mut unit,
                        &mut identity,
                        &mut subfactor,
                        &mut reject,
                    )
                };
                let event = if hr >= 0 {
                    if is_user(&identity, &me) {
                        FingerEvent::Matched
                    } else {
                        FingerEvent::NoMatch
                    }
                } else if hr == WINBIO_E_NO_MATCH || hr == WINBIO_E_BAD_CAPTURE {
                    FingerEvent::NoMatch
                } else {
                    FingerEvent::Unavailable
                };
                if tx.send(event).is_err() || event != FingerEvent::NoMatch {
                    break;
                }
            }
            unsafe { WinBioCloseSession(session) };
        })
        .expect("fingerprint thread");
    rx
}

fn is_user(identity: &WINBIO_IDENTITY, me: &[u8]) -> bool {
    if identity.Type != WINBIO_ID_TYPE_SID {
        return false;
    }
    let sid = unsafe { identity.Value.AccountSid };
    let len = (sid.Size as usize).min(sid.Data.len());
    &sid.Data[..len] == me
}

/// The signed-in user's SID, as bytes.
fn current_user_sid() -> Option<Vec<u8>> {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let mut buf = vec![0u8; 256];
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenUser,
            buf.as_mut_ptr().cast(),
            buf.len() as u32,
            &mut len,
        ) != 0;
        CloseHandle(token);
        if !ok {
            return None;
        }
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let sid = user.User.Sid;
        let n = GetLengthSid(sid) as usize;
        Some(std::slice::from_raw_parts(sid as *const u8, n).to_vec())
    }
}
