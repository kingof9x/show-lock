//! Turns off the Sticky Keys / Filter Keys / Toggle Keys shortcuts while
//! locked (pressing Shift five times would otherwise pop up a Windows
//! dialog), and puts them back afterwards. Changes are not saved to the
//! user profile, so a crash can't leave them off for good.

use windows_sys::Win32::UI::Accessibility::{FILTERKEYS, SKF_HOTKEYACTIVE, STICKYKEYS, TOGGLEKEYS};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SystemParametersInfoW, FKF_HOTKEYACTIVE, SPI_GETFILTERKEYS, SPI_GETSTICKYKEYS,
    SPI_GETTOGGLEKEYS, SPI_SETFILTERKEYS, SPI_SETSTICKYKEYS, SPI_SETTOGGLEKEYS, TKF_HOTKEYACTIVE,
};

pub struct Saved {
    sticky: STICKYKEYS,
    filter: FILTERKEYS,
    toggle: TOGGLEKEYS,
}

unsafe fn get<T>(action: u32, value: &mut T) {
    unsafe {
        SystemParametersInfoW(
            action,
            std::mem::size_of::<T>() as u32,
            (value as *mut T).cast(),
            0,
        )
    };
}

unsafe fn set<T>(action: u32, value: &T) {
    unsafe {
        SystemParametersInfoW(
            action,
            std::mem::size_of::<T>() as u32,
            (value as *const T).cast_mut().cast(),
            0,
        )
    };
}

pub fn disable_shortcuts() -> Saved {
    unsafe {
        let mut sticky = STICKYKEYS {
            cbSize: std::mem::size_of::<STICKYKEYS>() as u32,
            dwFlags: 0,
        };
        let mut filter = FILTERKEYS {
            cbSize: std::mem::size_of::<FILTERKEYS>() as u32,
            ..std::mem::zeroed()
        };
        let mut toggle = TOGGLEKEYS {
            cbSize: std::mem::size_of::<TOGGLEKEYS>() as u32,
            dwFlags: 0,
        };
        get(SPI_GETSTICKYKEYS, &mut sticky);
        get(SPI_GETFILTERKEYS, &mut filter);
        get(SPI_GETTOGGLEKEYS, &mut toggle);
        let saved = Saved {
            sticky,
            filter,
            toggle,
        };
        set(
            SPI_SETSTICKYKEYS,
            &STICKYKEYS {
                dwFlags: sticky.dwFlags & !SKF_HOTKEYACTIVE,
                ..sticky
            },
        );
        set(
            SPI_SETFILTERKEYS,
            &FILTERKEYS {
                dwFlags: filter.dwFlags & !FKF_HOTKEYACTIVE,
                ..filter
            },
        );
        set(
            SPI_SETTOGGLEKEYS,
            &TOGGLEKEYS {
                dwFlags: toggle.dwFlags & !TKF_HOTKEYACTIVE,
                ..toggle
            },
        );
        saved
    }
}

pub fn restore(saved: &Saved) {
    unsafe {
        set(SPI_SETSTICKYKEYS, &saved.sticky);
        set(SPI_SETFILTERKEYS, &saved.filter);
        set(SPI_SETTOGGLEKEYS, &saved.toggle);
    }
}
