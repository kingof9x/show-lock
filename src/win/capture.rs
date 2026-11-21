//! Monitor layout and the screenshot of the main screen.

use windows_sys::Win32::Foundation::{LPARAM, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
    EnumDisplayMonitors, GetDC, GetDIBits, GetMonitorInfoW, ReleaseDC, SelectObject, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, HDC, HMONITOR, MONITORINFO, SRCCOPY,
};
use windows_sys::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;

pub struct Shot {
    pub width: usize,
    pub height: usize,
    pub bgra: Vec<u8>,
}

/// (main screen, other screens), in physical pixels on the virtual desktop.
pub fn monitors() -> (RECT, Vec<RECT>) {
    unsafe extern "system" fn each(mon: HMONITOR, _dc: HDC, _r: *mut RECT, data: LPARAM) -> i32 {
        let list = unsafe { &mut *(data as *mut Vec<(RECT, bool)>) };
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if unsafe { GetMonitorInfoW(mon, &mut info) } != 0 {
            list.push((info.rcMonitor, info.dwFlags & MONITORINFOF_PRIMARY != 0));
        }
        1
    }
    let mut list: Vec<(RECT, bool)> = Vec::new();
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(each),
            &mut list as *mut _ as LPARAM,
        )
    };
    let primary = list
        .iter()
        .find(|(_, p)| *p)
        .map(|(r, _)| *r)
        .unwrap_or(RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        });
    let others = list
        .into_iter()
        .filter(|(_, p)| !p)
        .map(|(r, _)| r)
        .collect();
    (primary, others)
}

/// A copy of what the main screen shows right now.
pub fn primary_screenshot() -> Option<Shot> {
    let (r, _) = monitors();
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    if w <= 0 || h <= 0 {
        return None;
    }
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        let mem = CreateCompatibleDC(screen);
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, bmp);
        let copied = BitBlt(mem, 0, 0, w, h, screen, r.left, r.top, SRCCOPY | CAPTUREBLT) != 0;
        SelectObject(mem, old);

        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // top-down rows
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let mut bgra = vec![0u8; (w * h * 4) as usize];
        let rows = if copied {
            GetDIBits(
                mem,
                bmp,
                0,
                h as u32,
                bgra.as_mut_ptr().cast(),
                &mut info,
                DIB_RGB_COLORS,
            )
        } else {
            0
        };

        DeleteObject(bmp);
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);
        (rows == h).then_some(Shot {
            width: w as usize,
            height: h as usize,
            bgra,
        })
    }
}
