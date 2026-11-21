//! The lock screen itself: blurred background, clock, password card and
//! on-screen keyboard. Platform-neutral; `overlay.rs` wires it into Windows,
//! and `--preview` shows it in a normal window.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Event, FontId, Id, Key, Margin, Pos2, Rect, RichText,
    Sense, Stroke, TextureHandle, TextureOptions, Vec2,
};

use crate::blur::Image;
use crate::throttle::Throttle;
use crate::{clock, icon, style};

/// Result of a Windows Hello check.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))] // only Windows produces these
pub enum HelloOutcome {
    Verified,
    Cancelled,
    Failed(String),
}

/// Starts a Windows Hello check; the answer arrives on the receiver.
pub type HelloStarter = Box<dyn Fn() -> Receiver<HelloOutcome>>;
/// Called once the right password (or Windows Hello) unlocks the screen.
pub type UnlockFn = Box<dyn FnMut(&egui::Context)>;

/// Ignore input this soon after the lock screen appears (the keys of the lock
/// shortcut are often still being released).
const STARTUP_GRACE: Duration = Duration::from_millis(800);
const ACCENT: Color32 = Color32::from_rgb(96, 145, 255);
const ERROR: Color32 = Color32::from_rgb(255, 120, 120);

pub struct LockScreen {
    background: Option<TextureHandle>,
    background_image: Option<Image>,
    lock_icon: Option<TextureHandle>,
    password_hash: Option<String>,
    timeout: Duration,
    started: Instant,
    prompt_open: bool,
    last_activity: Instant,
    password: String,
    checking: Option<Receiver<bool>>,
    message: Option<(String, bool)>,
    shake_at: Option<Instant>,
    throttle: Throttle,
    keyboard: bool,
    shift: bool,
    hello: Option<HelloStarter>,
    hello_pending: Option<Receiver<HelloOutcome>>,
    on_unlock: UnlockFn,
    unlocked: bool,
    focus_password: bool,
}

impl LockScreen {
    pub fn new(
        background: Option<Image>,
        password_hash: Option<String>,
        timeout: Duration,
        hello: Option<HelloStarter>,
        on_unlock: UnlockFn,
    ) -> Self {
        let now = Instant::now();
        Self {
            background: None,
            background_image: background,
            lock_icon: None,
            password_hash,
            timeout,
            started: now,
            prompt_open: false,
            last_activity: now,
            password: String::new(),
            checking: None,
            message: None,
            shake_at: None,
            throttle: Throttle::default(),
            keyboard: false,
            shift: false,
            hello,
            hello_pending: None,
            on_unlock,
            unlocked: false,
            focus_password: false,
        }
    }

    /// Open the password card straight away (used by `--preview`).
    pub fn show_prompt(&mut self, keyboard: bool) {
        self.prompt_open = true;
        self.keyboard = keyboard;
        self.focus_password = true;
        self.last_activity = Instant::now();
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.load_textures(&ctx);
        self.poll_results(&ctx);
        if self.unlocked {
            return;
        }
        self.track_activity(&ctx);

        let rect = ctx.content_rect();
        let open = ctx.animate_bool_with_time(Id::unique("prompt"), self.prompt_open, 0.3);
        let kb = ctx.animate_bool_with_time(
            Id::unique("keyboard-shown"),
            self.prompt_open && self.keyboard,
            0.25,
        );
        self.paint_background(ui, rect, open);
        // The clock makes room for the on-screen keyboard.
        self.paint_clock(ui, rect, open, 1.0 - kb);
        if open > 0.0 {
            self.card(&ctx, open, kb);
            if self.keyboard {
                self.on_screen_keyboard(&ctx, open);
            }
        }
        // Keep the clock ticking and the countdowns moving.
        ctx.request_repaint_after(Duration::from_millis(if self.prompt_open {
            250
        } else {
            1000
        }));
    }

    fn load_textures(&mut self, ctx: &egui::Context) {
        if let Some(img) = self.background_image.take() {
            let color =
                egui::ColorImage::from_rgba_unmultiplied([img.width, img.height], &img.rgba);
            self.background = Some(ctx.load_texture("background", color, TextureOptions::LINEAR));
        }
        if self.lock_icon.is_none() {
            let size = 96;
            let color = egui::ColorImage::from_rgba_unmultiplied(
                [size, size],
                &icon::padlock_rgba(size, [255, 255, 255]),
            );
            self.lock_icon = Some(ctx.load_texture("lock-icon", color, TextureOptions::LINEAR));
        }
    }

    fn poll_results(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.checking {
            match rx.try_recv() {
                Ok(true) => {
                    self.checking = None;
                    self.unlock(ctx);
                }
                Ok(false) | Err(TryRecvError::Disconnected) => {
                    self.checking = None;
                    self.throttle.record_failure(Instant::now());
                    self.password.clear();
                    self.shake_at = Some(Instant::now());
                    self.message = Some(("Incorrect password".into(), true));
                    self.focus_password = true;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(rx) = &self.hello_pending {
            let outcome = match rx.try_recv() {
                Ok(o) => Some(o),
                Err(TryRecvError::Disconnected) => Some(HelloOutcome::Failed(
                    "Windows Hello stopped unexpectedly.".into(),
                )),
                Err(TryRecvError::Empty) => None,
            };
            if let Some(outcome) = outcome {
                self.hello_pending = None;
                self.last_activity = Instant::now();
                self.focus_password = true;
                match outcome {
                    HelloOutcome::Verified => self.unlock(ctx),
                    HelloOutcome::Cancelled => {
                        self.message = Some(("Windows Hello was cancelled".into(), false))
                    }
                    HelloOutcome::Failed(e) => self.message = Some((e, true)),
                }
            }
        }
    }

    fn unlock(&mut self, ctx: &egui::Context) {
        self.unlocked = true;
        self.password.clear();
        (self.on_unlock)(ctx);
    }

    fn track_activity(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        let (active, touched, typed) = ctx.input(|i| {
            let mut active = false;
            let mut touched = false;
            let mut typed = String::new();
            for e in &i.events {
                match e {
                    Event::Key { pressed: true, .. }
                    | Event::PointerButton { pressed: true, .. }
                    | Event::MouseWheel { .. }
                    | Event::PointerMoved(_) => active = true,
                    Event::Touch { .. } => {
                        active = true;
                        touched = true;
                    }
                    Event::Text(t) => {
                        active = true;
                        typed.push_str(t);
                    }
                    _ => {}
                }
            }
            (active, touched, typed)
        });
        if now.duration_since(self.started) < STARTUP_GRACE {
            return;
        }
        if active {
            self.last_activity = now;
            if touched {
                self.keyboard = true;
            }
            if !self.prompt_open {
                self.prompt_open = true;
                self.focus_password = true;
                // Keep the key that woke the screen as the first character.
                if self.throttle.remaining(now).is_none() {
                    self.password.push_str(&typed);
                }
            }
        }
        let busy = self.checking.is_some() || self.hello_pending.is_some();
        if self.prompt_open && !busy && now.duration_since(self.last_activity) > self.timeout {
            self.prompt_open = false;
            self.password.clear();
            self.message = None;
            self.keyboard = false;
        }
        if self.prompt_open && ctx.input(|i| i.key_pressed(Key::Escape)) && !busy {
            self.prompt_open = false;
            self.password.clear();
            self.message = None;
        }
    }

    fn paint_background(&self, ui: &egui::Ui, rect: Rect, open: f32) {
        let painter = ui.painter();
        match &self.background {
            Some(tex) => {
                // Cover the screen while keeping the picture's proportions.
                let size = tex.size_vec2();
                let scale = (rect.width() / size.x).max(rect.height() / size.y);
                let draw = Rect::from_center_size(rect.center(), size * scale);
                painter.image(
                    tex.id(),
                    draw,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::from_gray(205),
                );
            }
            None => style::paint_gradient(
                painter,
                rect,
                Color32::from_rgb(28, 34, 58),
                Color32::from_rgb(10, 12, 20),
            ),
        }
        // Darken towards the bottom, and more while the prompt is open.
        let top = Color32::from_black_alpha((60.0 + 70.0 * open) as u8);
        let bottom = Color32::from_black_alpha((150.0 + 50.0 * open) as u8);
        style::paint_gradient(painter, rect, top, bottom);
    }

    fn paint_clock(&self, ui: &egui::Ui, rect: Rect, open: f32, opacity: f32) {
        if opacity <= 0.0 {
            return;
        }
        let mut painter = ui.painter().clone();
        painter.set_opacity(opacity);
        let (time, date) = clock::now_strings();
        let lerp = |a: f32, b: f32| a + (b - a) * open;
        let centre_y = lerp(
            rect.center().y - 40.0,
            rect.top() + rect.height() * 0.13 + 30.0,
        );
        let size = lerp(120.0, 64.0);
        painter.text(
            Pos2::new(rect.center().x, centre_y),
            Align2::CENTER_CENTER,
            time,
            FontId::new(size, style::light_family()),
            Color32::WHITE,
        );
        painter.text(
            Pos2::new(rect.center().x, centre_y + size * 0.62),
            Align2::CENTER_TOP,
            date,
            FontId::proportional(lerp(24.0, 18.0)),
            Color32::from_white_alpha(220),
        );
        let hint_alpha = ((1.0 - open) * 170.0) as u8;
        if hint_alpha > 0 {
            painter.text(
                Pos2::new(rect.center().x, rect.bottom() - 64.0),
                Align2::CENTER_CENTER,
                "Press any key or move the mouse to unlock",
                FontId::proportional(16.0),
                Color32::from_white_alpha(hint_alpha),
            );
        }
    }

    fn card(&mut self, ctx: &egui::Context, open: f32, kb: f32) {
        let shake = self
            .shake_at
            .map(|t| t.elapsed().as_secs_f32())
            .filter(|t| *t < 0.45)
            .map_or(0.0, |t| (t * 45.0).sin() * 12.0 * (1.0 - t / 0.45));
        if shake != 0.0 {
            ctx.request_repaint();
        }
        let rise = (1.0 - open) * 24.0;
        // Centred, or above the on-screen keyboard (about 300 points tall).
        let keyboard_lift = 30.0 + (-165.0 - 30.0) * kb;
        let now = Instant::now();
        let blocked = self.throttle.remaining(now);
        let busy = self.checking.is_some() || self.hello_pending.is_some();

        egui::Area::new(Id::unique("card"))
            .anchor(
                Align2::CENTER_CENTER,
                Vec2::new(shake, keyboard_lift + rise),
            )
            .movable(false)
            .show(ctx, |ui| {
                ui.multiply_opacity(open);
                egui::Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(22, 24, 32, 215))
                    .stroke(Stroke::new(1.0, Color32::from_white_alpha(28)))
                    .corner_radius(CornerRadius::same(20))
                    .inner_margin(Margin::same(30))
                    .shadow(egui::Shadow {
                        offset: [0, 12],
                        blur: 40,
                        spread: 0,
                        color: Color32::from_black_alpha(110),
                    })
                    .show(ui, |ui| {
                        let width = 340.0;
                        ui.set_width(width);
                        ui.vertical_centered(|ui| {
                            if let Some(icon) = &self.lock_icon {
                                ui.add(
                                    egui::Image::new((icon.id(), Vec2::splat(40.0)))
                                        .tint(Color32::from_white_alpha(235)),
                                );
                            }
                            ui.add_space(10.0);
                            ui.label(RichText::new("Locked").size(26.0).color(Color32::WHITE));
                            ui.add_space(2.0);
                            ui.label(
                                RichText::new("Enter your password to unlock")
                                    .size(14.0)
                                    .color(Color32::from_white_alpha(170)),
                            );
                            ui.add_space(20.0);

                            let edit = egui::TextEdit::singleline(&mut self.password)
                                .password(true)
                                .hint_text("Password")
                                .font(FontId::proportional(18.0))
                                .margin(Margin::symmetric(12, 10))
                                .desired_width(width)
                                .interactive(blocked.is_none() && !busy);
                            let resp = ui.add(edit);
                            if self.focus_password && blocked.is_none() && !busy {
                                resp.request_focus();
                                self.focus_password = false;
                            }
                            let enter =
                                resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                            if resp.changed() {
                                self.message = None;
                            }

                            ui.add_space(14.0);
                            let label = if self.checking.is_some() {
                                "Checking…"
                            } else {
                                "Unlock"
                            };
                            let unlock_btn = egui::Button::new(
                                RichText::new(label).size(16.0).color(Color32::WHITE),
                            )
                            .fill(ACCENT)
                            .min_size(Vec2::new(width, 42.0));
                            let clicked = ui
                                .add_enabled(
                                    blocked.is_none() && !busy && !self.password.is_empty(),
                                    unlock_btn,
                                )
                                .clicked();
                            if (clicked || enter) && !busy {
                                self.submit();
                            }

                            if self.hello.is_some() {
                                ui.add_space(8.0);
                                let label = if self.hello_pending.is_some() {
                                    "Waiting for Windows Hello…"
                                } else {
                                    "Use Windows Hello"
                                };
                                let btn = egui::Button::new(RichText::new(label).size(15.0))
                                    .fill(Color32::from_white_alpha(18))
                                    .min_size(Vec2::new(width, 40.0));
                                if ui.add_enabled(!busy, btn).clicked() {
                                    self.start_hello();
                                }
                            }

                            ui.add_space(12.0);
                            let (text, color) = match (blocked, &self.message) {
                                (Some(left), _) => (
                                    format!(
                                        "Too many attempts. Try again in {}",
                                        fmt_duration(left)
                                    ),
                                    ERROR,
                                ),
                                (None, Some((m, true))) => (m.clone(), ERROR),
                                (None, Some((m, false))) => {
                                    (m.clone(), Color32::from_white_alpha(190))
                                }
                                (None, None) => (String::new(), Color32::TRANSPARENT),
                            };
                            ui.label(
                                RichText::new(if text.is_empty() {
                                    " ".to_string()
                                } else {
                                    text
                                })
                                .size(14.0)
                                .color(color),
                            );

                            ui.add_space(4.0);
                            let kb_label = if self.keyboard {
                                "Hide on-screen keyboard"
                            } else {
                                "On-screen keyboard"
                            };
                            let kb = ui.add(
                                egui::Label::new(
                                    RichText::new(kb_label)
                                        .size(13.0)
                                        .color(Color32::from_white_alpha(150)),
                                )
                                .sense(Sense::click()),
                            );
                            if kb.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                self.keyboard = !self.keyboard;
                                self.focus_password = true;
                            }
                        });
                    });
            });
    }

    fn submit(&mut self) {
        if self.password.is_empty() || self.throttle.remaining(Instant::now()).is_some() {
            return;
        }
        let password = std::mem::take(&mut self.password);
        let hash = self.password_hash.clone();
        let (tx, rx) = mpsc::channel();
        // Argon2 takes a moment; keep the UI responsive meanwhile.
        std::thread::spawn(move || {
            let ok = match hash {
                Some(h) => crate::config::verify_password(&h, &password),
                // Only `--preview` runs without a configured password.
                None => password == "demo",
            };
            let _ = tx.send(ok);
        });
        self.checking = Some(rx);
        self.message = None;
    }

    fn start_hello(&mut self) {
        if let Some(start) = &self.hello {
            self.hello_pending = Some(start());
            self.message = None;
        }
    }

    fn on_screen_keyboard(&mut self, ctx: &egui::Context, open: f32) {
        let rows: [&str; 4] = if self.shift {
            ["!@#$%^&*()", "QWERTYUIOP", "ASDFGHJKL", "ZXCVBNM-_."]
        } else {
            ["1234567890", "qwertyuiop", "asdfghjkl", "zxcvbnm-_."]
        };
        let busy = self.checking.is_some()
            || self.hello_pending.is_some()
            || self.throttle.remaining(Instant::now()).is_some();
        let key_size = Vec2::new(52.0, 50.0);
        let mut pressed: Option<KbKey> = None;
        egui::Area::new(Id::unique("keyboard"))
            .anchor(
                Align2::CENTER_BOTTOM,
                Vec2::new(0.0, -28.0 + (1.0 - open) * 30.0),
            )
            .movable(false)
            .show(ctx, |ui| {
                ui.multiply_opacity(open);
                egui::Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(22, 24, 32, 215))
                    .stroke(Stroke::new(1.0, Color32::from_white_alpha(28)))
                    .corner_radius(CornerRadius::same(18))
                    .inner_margin(Margin::same(14))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing = Vec2::splat(6.0);
                        for row in rows {
                            ui.horizontal(|ui| {
                                // Centre shorter rows.
                                let full = 10.0 * key_size.x + 9.0 * 6.0;
                                let this = row.chars().count() as f32 * key_size.x
                                    + (row.chars().count() as f32 - 1.0) * 6.0;
                                ui.add_space((full - this) / 2.0);
                                for c in row.chars() {
                                    if key_button(ui, &c.to_string(), key_size, false).clicked() {
                                        pressed = Some(KbKey::Char(c));
                                    }
                                }
                            });
                        }
                        ui.horizontal(|ui| {
                            if key_button(ui, "Shift", Vec2::new(110.0, key_size.y), self.shift)
                                .clicked()
                            {
                                pressed = Some(KbKey::Shift);
                            }
                            if key_button(ui, "Space", Vec2::new(250.0, key_size.y), false)
                                .clicked()
                            {
                                pressed = Some(KbKey::Char(' '));
                            }
                            if key_button(ui, "Delete", Vec2::new(90.0, key_size.y), false)
                                .clicked()
                            {
                                pressed = Some(KbKey::Backspace);
                            }
                            if key_button(ui, "Enter", Vec2::new(110.0, key_size.y), false)
                                .clicked()
                            {
                                pressed = Some(KbKey::Enter);
                            }
                        });
                    });
            });
        if let Some(k) = pressed {
            self.last_activity = Instant::now();
            if busy {
                return;
            }
            self.message = None;
            match k {
                KbKey::Char(c) => {
                    self.password.push(c);
                    self.shift = false;
                }
                KbKey::Shift => self.shift = !self.shift,
                KbKey::Backspace => {
                    self.password.pop();
                }
                KbKey::Enter => self.submit(),
            }
        }
    }
}

enum KbKey {
    Char(char),
    Shift,
    Backspace,
    Enter,
}

fn key_button(ui: &mut egui::Ui, label: &str, size: Vec2, active: bool) -> egui::Response {
    let fill = if active {
        ACCENT
    } else {
        Color32::from_white_alpha(22)
    };
    ui.add(
        egui::Button::new(RichText::new(label).size(18.0).color(Color32::WHITE))
            .fill(fill)
            .min_size(size)
            .corner_radius(CornerRadius::same(10)),
    )
}

fn fmt_duration(d: Duration) -> String {
    let s = d.as_secs() + u64::from(d.subsec_nanos() > 0);
    format!("{}:{:02}", s / 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countdown_rounds_up() {
        assert_eq!(fmt_duration(Duration::from_millis(29_100)), "0:30");
        assert_eq!(fmt_duration(Duration::from_secs(90)), "1:30");
    }
}
