//! `show-lock --preview`: the lock screen in an ordinary window, with no
//! input blocking, so the look can be checked safely.
//!
//! Unlocks with the configured password, or "demo" if none is set.

use std::time::Duration;

use eframe::egui;

use crate::blur;
use crate::config::Config;
use crate::lockscreen::LockScreen;

pub fn run() {
    let config = Config::load();
    let background = background();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Show Lock preview")
            .with_inner_size([1280.0, 800.0]),
        ..Default::default()
    };
    let _ = eframe::run_native(
        "Show Lock preview",
        options,
        Box::new(move |cc| {
            crate::style::setup(&cc.egui_ctx);
            let screen = LockScreen::new(
                Some(background),
                config.password_hash.clone(),
                Duration::from_secs(u64::from(config.prompt_timeout_secs)),
                preview_hello(&config, cc),
                Box::new(|ctx| ctx.send_viewport_cmd(egui::ViewportCommand::Close)),
            );
            let mut screen = screen;
            match std::env::var("SHOW_LOCK_PREVIEW_PROMPT").as_deref() {
                Ok("prompt") => screen.show_prompt(false),
                Ok("keyboard") => screen.show_prompt(true),
                _ => {}
            }
            Ok(Box::new(Preview { screen, frames: 0 }))
        }),
    );
}

struct Preview {
    screen: LockScreen,
    frames: u32,
}

impl eframe::App for Preview {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.screen.ui(ui);
        dev_screenshot(ui.ctx(), &mut self.frames);
    }
}

/// The real screen on Windows; a colourful sample elsewhere.
fn background() -> blur::Image {
    #[cfg(windows)]
    if let Some(shot) = crate::win::capture::primary_screenshot() {
        return blur::blurred_background(&shot.bgra, shot.width, shot.height);
    }
    let (w, h) = (640usize, 400usize);
    let mut bgra = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            let window = (0.1..0.55).contains(&fx) && (0.15..0.7).contains(&fy);
            let bar = fy > 0.92;
            let (r, g, b) = if bar {
                (30.0, 30.0, 36.0)
            } else if window {
                (235.0, 235.0, 240.0)
            } else {
                (40.0 + 120.0 * fx, 70.0 + 60.0 * fy, 160.0 - 60.0 * fx)
            };
            bgra[i] = b as u8;
            bgra[i + 1] = g as u8;
            bgra[i + 2] = r as u8;
            bgra[i + 3] = 255;
        }
    }
    blur::blurred_background(&bgra, w, h)
}

#[cfg(windows)]
fn preview_hello(
    config: &Config,
    cc: &eframe::CreationContext<'_>,
) -> Option<crate::lockscreen::HelloStarter> {
    config
        .windows_hello
        .then(|| crate::win::hello::starter(crate::win::hwnd_of(cc)))
}

#[cfg(not(windows))]
fn preview_hello(
    config: &Config,
    _cc: &eframe::CreationContext<'_>,
) -> Option<crate::lockscreen::HelloStarter> {
    config
        .windows_hello
        .then(|| -> crate::lockscreen::HelloStarter {
            Box::new(|| {
                let (tx, rx) = std::sync::mpsc::channel();
                let _ = tx.send(crate::lockscreen::HelloOutcome::Failed(
                    "Windows Hello needs Windows.".into(),
                ));
                rx
            })
        })
}

/// Development aid: with `SHOW_LOCK_SCREENSHOT=out.ppm`, save the window to
/// that file after a few frames and quit.
pub fn dev_screenshot(ctx: &egui::Context, frames: &mut u32) {
    let Some(path) = std::env::var_os("SHOW_LOCK_SCREENSHOT") else {
        return;
    };
    *frames += 1;
    let wait: u32 = std::env::var("SHOW_LOCK_SCREENSHOT_FRAMES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    if *frames == wait {
        let ctx2 = ctx.clone();
        ctx.request_screenshot(move |img| {
            let mut out = format!("P6\n{} {}\n255\n", img.size[0], img.size[1]).into_bytes();
            for p in &img.pixels {
                out.extend_from_slice(&[p.r(), p.g(), p.b()]);
            }
            let _ = std::fs::write(path, out);
            ctx2.send_viewport_cmd(egui::ViewportCommand::Close);
            ctx2.request_repaint();
        });
    }
    ctx.request_repaint();
}
