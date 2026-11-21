//! `show-lock --overlay`: the real lock screen. Started by the tray app.
//!
//! Exit codes the tray app acts on: 0 = unlocked, 2 = no password set.
//! Anything else (including a crash) makes the tray app lock Windows for real.

use std::time::Duration;

use eframe::egui;

use super::{capture, guard, hello};
use crate::blur;
use crate::config::Config;
use crate::lockscreen::LockScreen;

pub const EXIT_UNLOCKED: i32 = 0;
pub const EXIT_NO_PASSWORD: i32 = 2;
const EXIT_CLOSED: i32 = 3;

pub fn run() {
    super::set_dpi_aware();
    let config = Config::load();
    let Some(hash) = config.password_hash.clone() else {
        std::process::exit(EXIT_NO_PASSWORD)
    };

    // Capture before our own window exists, so it isn't in the picture.
    let (primary, others) = capture::monitors();
    let background =
        capture::primary_screenshot().map(|s| blur::blurred_background(&s.bgra, s.width, s.height));
    guard::start(primary, others);
    watch_tray();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Show Lock")
            .with_decorations(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_resizable(false)
            .with_active(true)
            .with_position([0.0, 0.0])
            .with_inner_size([800.0, 600.0]),
        ..Default::default()
    };
    let _ = eframe::run_native(
        "Show Lock",
        options,
        Box::new(move |cc| {
            crate::style::setup(&cc.egui_ctx);
            let hwnd = super::hwnd_of(cc);
            if let Some(h) = hwnd {
                guard::set_overlay(h);
            }
            let hello = (config.windows_hello && hwnd.is_some()).then(|| hello::starter(hwnd));
            let screen = LockScreen::new(
                background,
                Some(hash),
                Duration::from_secs(u64::from(config.prompt_timeout_secs.max(5))),
                hello,
                Box::new(|_| {
                    guard::release();
                    std::process::exit(EXIT_UNLOCKED);
                }),
            );
            Ok(Box::new(Overlay { screen }))
        }),
    );
    // The window went away without an unlock.
    guard::release();
    std::process::exit(EXIT_CLOSED);
}

/// If the tray app that started us dies while we're up (for example someone
/// ended it in Task Manager), lock Windows for real. The tray app does the
/// same if we die, so neither can be ended alone to get in.
fn watch_tray() {
    use windows_sys::Win32::System::Shutdown::LockWorkStation;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, INFINITE, PROCESS_SYNCHRONIZE,
    };
    let Some(pid) = std::env::args().nth(2).and_then(|a| a.parse::<u32>().ok()) else {
        return;
    };
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        return;
    }
    let handle = handle as isize;
    std::thread::spawn(move || unsafe {
        WaitForSingleObject(handle as _, INFINITE);
        LockWorkStation();
    });
}

struct Overlay {
    screen: LockScreen,
}

impl eframe::App for Overlay {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if ui.ctx().input(|i| i.viewport().close_requested()) {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        self.screen.ui(ui);
    }
}
