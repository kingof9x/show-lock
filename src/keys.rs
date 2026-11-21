//! Which keys the lock screen always swallows, whatever window has focus.
//! Kept free of Windows types so it can be unit-tested anywhere.

// Only the Windows build uses these outside tests.
#![cfg_attr(not(windows), allow(dead_code))]

pub const VK_TAB: u32 = 0x09;
pub const VK_ESCAPE: u32 = 0x1B;
pub const VK_SPACE: u32 = 0x20;
pub const VK_SNAPSHOT: u32 = 0x2C;
pub const VK_LWIN: u32 = 0x5B;
pub const VK_RWIN: u32 = 0x5C;
pub const VK_APPS: u32 = 0x5D;
pub const VK_SLEEP: u32 = 0x5F;
pub const VK_F4: u32 = 0x73;
pub const VK_F6: u32 = 0x76;
/// Browser, volume, media and app-launch keys.
pub const VK_BROWSER_BACK: u32 = 0xA6;
pub const VK_LAUNCH_APP2: u32 = 0xB7;

/// True for keys and combinations that would switch windows, open the Start
/// menu or Task Manager, take a screenshot, or control the media playing on
/// the other screens.
pub fn is_system_key(vk: u32, alt: bool, ctrl: bool) -> bool {
    match vk {
        VK_LWIN | VK_RWIN | VK_APPS | VK_SLEEP | VK_SNAPSHOT => true,
        VK_BROWSER_BACK..=VK_LAUNCH_APP2 => true,
        VK_TAB | VK_F4 | VK_F6 | VK_SPACE => alt,
        VK_ESCAPE => alt || ctrl,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_switching_and_media() {
        assert!(is_system_key(VK_LWIN, false, false));
        assert!(is_system_key(VK_TAB, true, false)); // Alt+Tab
        assert!(is_system_key(VK_F4, true, false)); // Alt+F4
        assert!(is_system_key(VK_ESCAPE, false, true)); // Ctrl+Esc, Ctrl+Shift+Esc
        assert!(is_system_key(0xAF, false, false)); // volume up
        assert!(is_system_key(0xB3, false, false)); // play/pause
    }

    #[test]
    fn lets_typing_through() {
        assert!(!is_system_key(VK_TAB, false, false));
        assert!(!is_system_key(VK_ESCAPE, false, false));
        assert!(!is_system_key(VK_SPACE, false, false));
        assert!(!is_system_key(u32::from(b'A'), false, false));
        assert!(!is_system_key(0x0D, false, false)); // Enter
        assert!(!is_system_key(0x08, false, false)); // Backspace
    }
}
