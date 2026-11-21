//! Everything that only exists on Windows.

pub mod accessibility;
pub mod capture;
pub mod fingerprint;
pub mod guard;
pub mod hello;
pub mod overlay;
pub mod startup;
pub mod tray;

use windows_sys::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};

/// UTF-16, nul-terminated, for Win32 calls.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Work in real pixels on every monitor (needed for exact screen positions
/// and full-resolution captures). Must run before any window exists.
pub fn set_dpi_aware() {
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
}

/// The native window handle of an eframe window.
pub fn hwnd_of(cc: &eframe::CreationContext<'_>) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match cc.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}
