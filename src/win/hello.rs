//! Windows Hello (fingerprint, face or Windows PIN) via UserConsentVerifier.

use std::sync::mpsc;

use windows::core::{factory, HSTRING};
use windows::Security::Credentials::UI::{
    UserConsentVerificationResult, UserConsentVerifier, UserConsentVerifierAvailability,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::WinRT::IUserConsentVerifierInterop;
use windows_future::IAsyncOperation;

use crate::lockscreen::{HelloOutcome, HelloStarter};

/// True when Windows Hello is set up for this user on this PC.
pub fn available() -> bool {
    UserConsentVerifier::CheckAvailabilityAsync()
        .and_then(|op| op.join())
        .map(|a| a == UserConsentVerifierAvailability::Available)
        .unwrap_or(false)
}

/// A starter for the lock screen: opens the Windows Hello prompt over
/// `hwnd` and reports the result.
pub fn starter(hwnd: Option<isize>) -> HelloStarter {
    Box::new(move || {
        let (tx, rx) = mpsc::channel();
        super::guard::set_hello_active(true);
        std::thread::spawn(move || {
            let outcome = verify(hwnd);
            super::guard::set_hello_active(false);
            let _ = tx.send(outcome);
        });
        rx
    })
}

fn verify(hwnd: Option<isize>) -> HelloOutcome {
    let message = HSTRING::from("Unlock Show Lock");
    let op: windows::core::Result<IAsyncOperation<UserConsentVerificationResult>> = match hwnd {
        Some(h) => factory::<UserConsentVerifier, IUserConsentVerifierInterop>().and_then(
            |interop| unsafe { interop.RequestVerificationForWindowAsync(HWND(h as _), &message) },
        ),
        None => UserConsentVerifier::RequestVerificationAsync(&message),
    };
    match op.and_then(|op| op.join()) {
        Ok(UserConsentVerificationResult::Verified) => HelloOutcome::Verified,
        Ok(UserConsentVerificationResult::Canceled) => HelloOutcome::Cancelled,
        _ => HelloOutcome::Failed,
    }
}
