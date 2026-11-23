//! Small things the settings window needs from the OS, with harmless
//! stand-ins off Windows (only used for development previews there).

/// Start this program again with one argument, without waiting for it.
pub fn spawn_self(arg: &str) {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).arg(arg).spawn();
    }
}

#[cfg(windows)]
pub use crate::win::hello::available as hello_available;
#[cfg(windows)]
pub use crate::win::startup::{enabled as startup_enabled, set as set_startup};

#[cfg(not(windows))]
pub fn hello_available() -> bool {
    false
}

#[cfg(not(windows))]
pub fn startup_enabled() -> bool {
    false
}

#[cfg(not(windows))]
pub fn set_startup(_on: bool) -> Result<(), String> {
    Ok(())
}
