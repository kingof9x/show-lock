//! Show Lock: a lock screen for Windows that leaves other displays playing.
//!
//! One program, several roles (picked by the first argument):
//! - no argument: the tray app. Owns the shortcut, starts the lock screen and
//!   locks Windows for real if the lock screen ever dies unexpectedly.
//! - `--overlay`: the lock screen itself (started by the tray app).
//! - `--settings`: the settings window.
//! - `--preview`: the lock screen in a normal window, nothing blocked.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod blur;
mod config;
mod icon;
mod keys;
mod lockscreen;
mod platform;
mod preview;
mod settings;
mod style;
mod throttle;
#[cfg(windows)]
mod win;

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_default();
    match arg.as_str() {
        "--preview" => preview::run(),
        "--settings" => settings::run(),
        #[cfg(windows)]
        "--overlay" => win::overlay::run(),
        #[cfg(windows)]
        "" => win::tray::run(),
        "--help" | "-h" => println!(
            "show-lock [--settings | --preview]\nWith no argument it starts in the system tray."
        ),
        other => {
            #[cfg(not(windows))]
            eprintln!("Show Lock runs on Windows. Only --preview and --settings work here.");
            if !other.is_empty() {
                eprintln!("Unknown option: {other}");
            }
            std::process::exit(1);
        }
    }
}
