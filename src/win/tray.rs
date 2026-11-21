//! The tray app (no argument): tray icon, lock shortcut, starts the lock
//! screen and watches it.
//!
//! While locked it keeps the PC and displays awake and turns off the
//! accessibility shortcuts. If the lock screen dies without being unlocked,
//! it locks Windows for real. If Windows itself gets locked and then
//! unlocked with the Windows password, that counts as unlocking.

use std::cell::RefCell;
use std::os::windows::io::AsRawHandle;
use std::process::{Child, Command};

use windows_sys::Win32::Foundation::{
    CloseHandle, DuplicateHandle, GetLastError, DUPLICATE_SAME_ACCESS, ERROR_ALREADY_EXISTS,
    HANDLE, HWND, LPARAM, LRESULT, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Power::{
    SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED,
};
use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows_sys::Win32::System::RemoteDesktop::{
    WTSRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION,
};
use windows_sys::Win32::System::Shutdown::LockWorkStation;
use windows_sys::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcess, GetExitCodeProcess, WaitForSingleObject, INFINITE,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN,
};
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use super::overlay::{EXIT_NO_PASSWORD, EXIT_UNLOCKED};
use super::{accessibility, wide};
use crate::config::Config;

const WM_TRAY: u32 = WM_APP + 1;
const WM_OVERLAY_EXIT: u32 = WM_APP + 2;
const WM_SETTINGS_EXIT: u32 = WM_APP + 3;
const WTS_SESSION_UNLOCK: usize = 0x8;
const HOTKEY_ID: i32 = 1;
const MENU_LOCK: usize = 1;
const MENU_SETTINGS: usize = 2;
const MENU_QUIT: usize = 3;

struct State {
    hwnd: HWND,
    config: Config,
    overlay: Option<Child>,
    /// Set when we end the lock screen ourselves (Windows was unlocked).
    disarming: bool,
    saved_accessibility: Option<accessibility::Saved>,
    settings_open: bool,
    hotkey_ok: bool,
    taskbar_created: u32,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.borrow_mut().as_mut().map(f))
}

pub fn run() {
    super::set_dpi_aware();
    let name = wide("Local\\ShowLock.Tray");
    unsafe {
        CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            // Already running: opening it again shows the settings.
            crate::platform::spawn_self("--settings");
            return;
        }
    }

    let hwnd = unsafe { create_window() };
    let config = Config::load();
    let taskbar_created = unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) };
    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            hwnd,
            config,
            overlay: None,
            disarming: false,
            saved_accessibility: None,
            settings_open: false,
            hotkey_ok: false,
            taskbar_created,
        })
    });
    unsafe {
        WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION);
    }
    add_icon();
    register_hotkey();
    if !with_state(|s| s.config.has_password()).unwrap_or(false) {
        open_settings();
    } else if !with_state(|s| s.hotkey_ok).unwrap_or(true) {
        let label = with_state(|s| s.config.hotkey.label()).unwrap_or_default();
        notify(
            "Shortcut unavailable",
            &format!("{label} is used by another app. Pick another in Show Lock settings."),
        );
    }

    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe fn create_window() -> HWND {
    unsafe {
        let module = GetModuleHandleW(std::ptr::null());
        let class = wide("ShowLockTray");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: module,
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);
        CreateWindowExW(
            0,
            class.as_ptr(),
            wide("Show Lock").as_ptr(),
            0,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            module,
            std::ptr::null(),
        )
    }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_HOTKEY if wparam as i32 == HOTKEY_ID => lock(),
        WM_TRAY => match lparam as u32 {
            WM_LBUTTONDBLCLK => lock(),
            WM_RBUTTONUP | WM_LBUTTONUP | WM_CONTEXTMENU => show_menu(hwnd),
            _ => {}
        },
        WM_OVERLAY_EXIT => overlay_exited(wparam as u32),
        WM_SETTINGS_EXIT => settings_closed(),
        WM_WTSSESSION_CHANGE if wparam == WTS_SESSION_UNLOCK => disarm(),
        WM_DESTROY => unsafe { PostQuitMessage(0) },
        _ => {
            if with_state(|s| s.taskbar_created == msg).unwrap_or(false) {
                // Explorer restarted: put the icon back.
                add_icon();
            }
            return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        }
    }
    0
}

fn show_menu(hwnd: HWND) {
    let (label, locked) =
        with_state(|s| (s.config.hotkey.label(), s.overlay.is_some())).unwrap_or_default();
    unsafe {
        let menu = CreatePopupMenu();
        let lock_text = wide(&format!("Lock now\t{label}"));
        AppendMenuW(
            menu,
            if locked {
                MF_STRING | MF_GRAYED
            } else {
                MF_STRING
            },
            MENU_LOCK,
            lock_text.as_ptr(),
        );
        AppendMenuW(menu, MF_STRING, MENU_SETTINGS, wide("Settings…").as_ptr());
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        AppendMenuW(
            menu,
            if locked {
                MF_STRING | MF_GRAYED
            } else {
                MF_STRING
            },
            MENU_QUIT,
            wide("Quit Show Lock").as_ptr(),
        );
        SetMenuDefaultItem(menu, MENU_LOCK as u32, 0);
        let mut pt = std::mem::zeroed();
        GetCursorPos(&mut pt);
        // Needed so the menu closes when clicking elsewhere.
        SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
            pt.x,
            pt.y,
            0,
            hwnd,
            std::ptr::null(),
        ) as usize;
        DestroyMenu(menu);
        match cmd {
            MENU_LOCK => lock(),
            MENU_SETTINGS => open_settings(),
            MENU_QUIT => {
                remove_icon();
                DestroyWindow(hwnd);
            }
            _ => {}
        }
    }
}

fn lock() {
    let ready = with_state(|s| {
        if s.overlay.is_some() {
            return None;
        }
        s.config = Config::load();
        Some(s.config.has_password())
    })
    .flatten();
    match ready {
        None => {}
        Some(false) => {
            notify(
                "Set a password first",
                "Choose the password that unlocks the lock screen.",
            );
            open_settings();
        }
        Some(true) => start_overlay(),
    }
}

fn start_overlay() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let saved = accessibility::disable_shortcuts();
    unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED) };
    match Command::new(exe)
        .arg("--overlay")
        .arg(std::process::id().to_string())
        .spawn()
    {
        Ok(child) => {
            let hwnd = with_state(|s| s.hwnd as isize).unwrap_or(0);
            watch_process(&child, hwnd, WM_OVERLAY_EXIT);
            with_state(|s| {
                s.overlay = Some(child);
                s.disarming = false;
                s.saved_accessibility = Some(saved);
            });
        }
        Err(_) => {
            accessibility::restore(&saved);
            unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
            // Couldn't show our lock screen: fall back to the Windows one.
            unsafe { LockWorkStation() };
        }
    }
}

/// Post `msg` (with the exit code) to our window when `child` exits.
fn watch_process(child: &Child, hwnd: isize, msg: u32) {
    let mut dup: HANDLE = std::ptr::null_mut();
    let ok = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            child.as_raw_handle() as HANDLE,
            GetCurrentProcess(),
            &mut dup,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    } != 0;
    if !ok {
        return;
    }
    let dup = dup as isize;
    std::thread::spawn(move || unsafe {
        let h = dup as HANDLE;
        WaitForSingleObject(h, INFINITE);
        let mut code = 1u32;
        GetExitCodeProcess(h, &mut code);
        CloseHandle(h);
        PostMessageW(hwnd as HWND, msg, code as WPARAM, 0);
    });
}

fn overlay_exited(code: u32) {
    let Some((disarming, saved)) = with_state(|s| {
        if let Some(mut child) = s.overlay.take() {
            let _ = child.wait();
        }
        (
            std::mem::take(&mut s.disarming),
            s.saved_accessibility.take(),
        )
    }) else {
        return;
    };
    if let Some(saved) = saved {
        accessibility::restore(&saved);
    }
    unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
    if disarming || code == EXIT_UNLOCKED as u32 {
        return;
    }
    if code == EXIT_NO_PASSWORD as u32 {
        open_settings();
        return;
    }
    // The lock screen died without an unlock (crashed or was killed):
    // never leave the PC open.
    unsafe { LockWorkStation() };
}

/// Windows was unlocked with the Windows password: end our lock screen too.
fn disarm() {
    with_state(|s| {
        if let Some(child) = s.overlay.as_mut() {
            s.disarming = true;
            let _ = child.kill();
        }
    });
}

fn open_settings() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    if with_state(|s| s.settings_open).unwrap_or(false) {
        return;
    }
    if let Ok(child) = Command::new(exe).arg("--settings").spawn() {
        let hwnd = with_state(|s| s.hwnd as isize).unwrap_or(0);
        watch_process(&child, hwnd, WM_SETTINGS_EXIT);
        with_state(|s| s.settings_open = true);
        // The child is reaped by the watcher's handle; dropping ours is fine.
        drop(child);
    }
}

fn settings_closed() {
    with_state(|s| {
        s.settings_open = false;
        s.config = Config::load();
    });
    register_hotkey();
    update_tip();
}

fn register_hotkey() {
    with_state(|s| unsafe {
        UnregisterHotKey(s.hwnd, HOTKEY_ID);
        let hk = s.config.hotkey;
        let mut mods = MOD_NOREPEAT;
        if hk.ctrl {
            mods |= MOD_CONTROL;
        }
        if hk.alt {
            mods |= MOD_ALT;
        }
        if hk.shift {
            mods |= MOD_SHIFT;
        }
        if hk.win {
            mods |= MOD_WIN;
        }
        // Letters and digits have the same virtual-key codes as their ASCII.
        s.hotkey_ok =
            hk.validate().is_ok() && RegisterHotKey(s.hwnd, HOTKEY_ID, mods, hk.key as u32) != 0;
    });
}

// ---- tray icon -------------------------------------------------------------

fn icon_data(s: &State) -> NOTIFYICONDATAW {
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = s.hwnd;
    nid.uID = 1;
    nid
}

fn tip_text(s: &State) -> String {
    if s.config.has_password() {
        format!("Show Lock: press {} to lock", s.config.hotkey.label())
    } else {
        "Show Lock: set a password to start".into()
    }
}

fn copy_into(dst: &mut [u16], text: &str) {
    let max = dst.len() - 1;
    for (d, c) in dst
        .iter_mut()
        .zip(text.encode_utf16().take(max).chain(std::iter::once(0)))
    {
        *d = c;
    }
}

fn add_icon() {
    with_state(|s| unsafe {
        let mut nid = icon_data(s);
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_TRAY;
        nid.hIcon = make_icon();
        copy_into(&mut nid.szTip, &tip_text(s));
        Shell_NotifyIconW(NIM_ADD, &nid);
    });
}

fn update_tip() {
    with_state(|s| unsafe {
        let mut nid = icon_data(s);
        nid.uFlags = NIF_TIP;
        copy_into(&mut nid.szTip, &tip_text(s));
        Shell_NotifyIconW(NIM_MODIFY, &nid);
    });
}

fn remove_icon() {
    with_state(|s| unsafe {
        let nid = icon_data(s);
        Shell_NotifyIconW(NIM_DELETE, &nid);
    });
}

/// A Windows notification from the tray icon.
fn notify(title: &str, text: &str) {
    with_state(|s| unsafe {
        let mut nid = icon_data(s);
        nid.uFlags = NIF_INFO;
        copy_into(&mut nid.szInfoTitle, title);
        copy_into(&mut nid.szInfo, text);
        Shell_NotifyIconW(NIM_MODIFY, &nid);
    });
}

/// The padlock, white on a dark taskbar and dark on a light one.
unsafe fn make_icon() -> HICON {
    unsafe {
        let size = GetSystemMetrics(SM_CXSMICON).max(16) as usize;
        let rgb = if light_taskbar() {
            [32, 32, 32]
        } else {
            [255, 255, 255]
        };
        let rgba = crate::icon::padlock_rgba(size, rgb);
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size as i32,
            biHeight: -(size as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let color = CreateDIBSection(
            std::ptr::null_mut(),
            &info,
            DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        );
        if color.is_null() || bits.is_null() {
            return LoadIconW(std::ptr::null_mut(), IDI_SHIELD);
        }
        let dst = std::slice::from_raw_parts_mut(bits.cast::<u8>(), size * size * 4);
        for (d, s) in dst.chunks_exact_mut(4).zip(rgba.chunks_exact(4)) {
            d.copy_from_slice(&[s[2], s[1], s[0], s[3]]);
        }
        let mask = CreateBitmap(size as i32, size as i32, 1, 1, std::ptr::null());
        let ii = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let icon = CreateIconIndirect(&ii);
        DeleteObject(color);
        DeleteObject(mask);
        icon
    }
}

fn light_taskbar() -> bool {
    let key = wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let value = wide("SystemUsesLightTheme");
    let mut data = 0u32;
    let mut size = 4u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut data as *mut u32).cast(),
            &mut size,
        )
    };
    status == 0 && data == 1
}
