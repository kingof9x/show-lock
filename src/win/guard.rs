//! Input blocking while the lock screen is up (runs inside the lock screen
//! process, so input is blocked for exactly as long as it is alive).
//!
//! - Low-level keyboard hook: always swallows system keys (Win, Alt+Tab,
//!   Ctrl+Esc, media keys, …), keystrokes injected by other programs, and
//!   every key while some other window has focus.
//! - Low-level mouse hook: swallows clicks and wheel outside the main screen.
//! - The cursor is kept on the main screen.
//! - An invisible (1/255 opacity) window on each other screen catches clicks
//!   and touches, so the picture there is untouched but can't be used.
//! - A watcher keeps the lock screen on top and focused.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, AtomicU32, Ordering};
use std::time::Duration;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{GetStockObject, BLACK_BRUSH, HBRUSH};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{
    AttachThreadInput, GetCurrentProcessId, GetCurrentThreadId,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    keybd_event, GetAsyncKeyState, SetFocus, KEYEVENTF_KEYUP, VK_CONTROL, VK_MENU,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use super::wide;
use crate::keys;

static OWN_PID: AtomicU32 = AtomicU32::new(0);
static OVERLAY: AtomicIsize = AtomicIsize::new(0);
static HELLO_ACTIVE: AtomicBool = AtomicBool::new(false);
static PRIMARY: [AtomicI32; 4] = [
    AtomicI32::new(0),
    AtomicI32::new(0),
    AtomicI32::new(0),
    AtomicI32::new(0),
];
/// Marks the Alt tap we send ourselves to win back focus, so the hook lets it through.
const OWN_INPUT: usize = 0x5348_4C4B; // "SHLK"

fn primary() -> RECT {
    RECT {
        left: PRIMARY[0].load(Ordering::Relaxed),
        top: PRIMARY[1].load(Ordering::Relaxed),
        right: PRIMARY[2].load(Ordering::Relaxed),
        bottom: PRIMARY[3].load(Ordering::Relaxed),
    }
}

/// Start blocking. `others` are the screens to cover with click catchers.
pub fn start(primary: RECT, others: Vec<RECT>) {
    OWN_PID.store(unsafe { GetCurrentProcessId() }, Ordering::Relaxed);
    for (slot, v) in PRIMARY
        .iter()
        .zip([primary.left, primary.top, primary.right, primary.bottom])
    {
        slot.store(v, Ordering::Relaxed);
    }
    let rects: Vec<[i32; 4]> = others
        .iter()
        .map(|r| [r.left, r.top, r.right, r.bottom])
        .collect();
    std::thread::Builder::new()
        .name("guard".into())
        .spawn(move || guard_thread(rects))
        .expect("guard thread");
    std::thread::Builder::new()
        .name("focus".into())
        .spawn(focus_thread)
        .expect("focus thread");
}

/// Tell the guard which window is the lock screen, and put it over the main screen.
pub fn set_overlay(hwnd: isize) {
    OVERLAY.store(hwnd, Ordering::Relaxed);
    let r = primary();
    unsafe {
        SetWindowPos(
            hwnd as HWND,
            HWND_TOPMOST,
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            SWP_SHOWWINDOW,
        )
    };
}

/// While Windows Hello's own window is open, input may go to it.
pub fn set_hello_active(active: bool) {
    HELLO_ACTIVE.store(active, Ordering::Relaxed);
}

/// Free the cursor (call just before the lock screen exits).
pub fn release() {
    unsafe { ClipCursor(std::ptr::null()) };
}

/// Keys may reach the focused window only when it is the lock screen (or
/// the Windows Hello prompt the lock screen opened).
fn focus_is_ours() -> bool {
    if HELLO_ACTIVE.load(Ordering::Relaxed) {
        return true;
    }
    let fg = unsafe { GetForegroundWindow() };
    if fg.is_null() {
        return false;
    }
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(fg, &mut pid) };
    pid == OWN_PID.load(Ordering::Relaxed)
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let k = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        let injected = k.flags & LLKHF_INJECTED != 0;
        let ours = injected && k.dwExtraInfo == OWN_INPUT;
        // Releases always pass (except Win/Menu), so keys held while
        // locking (the shortcut's Ctrl/Alt) don't stay stuck down.
        let release = k.flags & LLKHF_UP != 0;
        let win_key = matches!(k.vkCode, keys::VK_LWIN | keys::VK_RWIN | keys::VK_APPS);
        if !ours && !(release && !win_key) {
            let alt = k.flags & LLKHF_ALTDOWN != 0;
            let ctrl = unsafe { GetAsyncKeyState(VK_CONTROL as i32) } < 0;
            if injected || keys::is_system_key(k.vkCode, alt, ctrl) || !focus_is_ours() {
                return 1;
            }
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && wparam as u32 != WM_MOUSEMOVE {
        let m = unsafe { &*(lparam as *const MSLLHOOKSTRUCT) };
        let r = primary();
        let POINT { x, y } = m.pt;
        let on_main = x >= r.left && x < r.right && y >= r.top && y < r.bottom;
        if !on_main {
            return 1;
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

unsafe extern "system" fn shield_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => MA_NOACTIVATEANDEAT as LRESULT,
        WM_CLOSE => 0,
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn guard_thread(others: Vec<[i32; 4]>) {
    unsafe {
        let module = GetModuleHandleW(std::ptr::null());
        let class = wide("ShowLockShield");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(shield_proc),
            hInstance: module,
            lpszClassName: class.as_ptr(),
            hbrBackground: GetStockObject(BLACK_BRUSH) as HBRUSH,
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);
        let title = wide("Show Lock");
        let mut shields = Vec::new();
        for [l, t, r, b] in others {
            let h = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class.as_ptr(),
                title.as_ptr(),
                WS_POPUP,
                l,
                t,
                r - l,
                b - t,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                module,
                std::ptr::null(),
            );
            if !h.is_null() {
                // 1/255 opacity: invisible, but still catches every click and touch.
                SetLayeredWindowAttributes(h, 0, 1, LWA_ALPHA);
                ShowWindow(h, SW_SHOWNOACTIVATE);
                shields.push(h);
            }
        }

        SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), module, 0);
        SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), module, 0);
        SetTimer(std::ptr::null_mut(), 0, 200, None);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            if msg.message == WM_TIMER && msg.hwnd.is_null() {
                let r = primary();
                ClipCursor(&r);
                for &h in &shields {
                    SetWindowPos(
                        h,
                        HWND_TOPMOST,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                }
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Keeps the lock screen topmost and focused. Separate from the hook
/// thread so a slow window elsewhere can never stall the hooks.
fn focus_thread() {
    loop {
        std::thread::sleep(Duration::from_millis(150));
        let hwnd = OVERLAY.load(Ordering::Relaxed) as HWND;
        if hwnd.is_null() || HELLO_ACTIVE.load(Ordering::Relaxed) {
            continue;
        }
        unsafe {
            SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
            let fg = GetForegroundWindow();
            if fg != hwnd {
                force_foreground(hwnd, fg);
            }
        }
    }
}

unsafe fn force_foreground(hwnd: HWND, fg: HWND) {
    unsafe {
        let me = GetCurrentThreadId();
        let fg_thread = if fg.is_null() {
            0
        } else {
            GetWindowThreadProcessId(fg, std::ptr::null_mut())
        };
        let attached =
            fg_thread != 0 && fg_thread != me && AttachThreadInput(me, fg_thread, 1) != 0;
        BringWindowToTop(hwnd);
        let mut ok = SetForegroundWindow(hwnd) != 0;
        if !ok {
            // Windows only lets the app that last had input take focus; a
            // tap of Alt counts as input from us.
            keybd_event(VK_MENU as u8, 0, 0, OWN_INPUT);
            keybd_event(VK_MENU as u8, 0, KEYEVENTF_KEYUP, OWN_INPUT);
            ok = SetForegroundWindow(hwnd) != 0;
        }
        if ok {
            SetFocus(hwnd);
        }
        if attached {
            AttachThreadInput(me, fg_thread, 0);
        }
    }
}
