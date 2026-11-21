//! The lock screen itself: a lightly blurred picture of the main screen and,
//! once input is detected, a single password box. Nothing else.
//!
//! Platform-neutral; `win/overlay.rs` wires it into Windows, and `--preview`
//! shows it in a normal window.

use std::f32::consts::PI;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Event, FontId, Id, Key, Margin, Pos2, Rect, RichText,
    Sense, Shape, Stroke, TextureHandle, TextureOptions, Vec2,
};

use crate::blur::Image;
use crate::throttle::Throttle;

/// Result of a Windows Hello prompt (the fallback when the fingerprint
/// reader can't be read directly).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))] // only Windows produces these
pub enum HelloOutcome {
    Verified,
    Cancelled,
    Failed,
}

/// What the fingerprint reader reports while the screen is locked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub enum FingerEvent {
    /// A finger of the signed-in Windows user.
    Matched,
    /// Someone else's finger, or a smudged read.
    NoMatch,
    /// The reader can't be used directly (none, or reserved by Windows).
    Unavailable,
}

/// Opens the Windows Hello prompt; the answer arrives on the receiver.
pub type HelloStarter = Box<dyn Fn() -> Receiver<HelloOutcome>>;
/// Called once the right password or fingerprint unlocks the screen.
pub type UnlockFn = Box<dyn FnMut(&egui::Context)>;

/// Ignore input this soon after the lock screen appears (the keys of the lock
/// shortcut are often still being released).
const STARTUP_GRACE: Duration = Duration::from_millis(800);
const FIELD_WIDTH: f32 = 300.0;
const ERROR: Color32 = Color32::from_rgb(255, 110, 110);

pub struct LockScreen {
    background: Option<TextureHandle>,
    background_image: Option<Image>,
    password_hash: Option<String>,
    timeout: Duration,
    started: Instant,
    prompt_open: bool,
    last_activity: Instant,
    password: String,
    checking: Option<Receiver<bool>>,
    shake_at: Option<Instant>,
    throttle: Throttle,
    keyboard: bool,
    shift: bool,
    fingerprint: Option<Receiver<FingerEvent>>,
    finger_live: bool,
    hello: Option<HelloStarter>,
    hello_pending: Option<Receiver<HelloOutcome>>,
    on_unlock: UnlockFn,
    unlocked: bool,
    focus_password: bool,
}

impl LockScreen {
    /// `fingerprint`: events from a reader that is already listening.
    /// `hello`: opens the Windows Hello prompt, offered only when the reader
    /// can't be listened to directly.
    pub fn new(
        background: Option<Image>,
        password_hash: Option<String>,
        timeout: Duration,
        fingerprint: Option<Receiver<FingerEvent>>,
        hello: Option<HelloStarter>,
        on_unlock: UnlockFn,
    ) -> Self {
        let now = Instant::now();
        Self {
            background: None,
            background_image: background,
            password_hash,
            timeout,
            started: now,
            prompt_open: false,
            last_activity: now,
            password: String::new(),
            checking: None,
            shake_at: None,
            throttle: Throttle::default(),
            keyboard: false,
            shift: false,
            finger_live: fingerprint.is_some(),
            fingerprint,
            hello,
            hello_pending: None,
            on_unlock,
            unlocked: false,
            focus_password: false,
        }
    }

    /// Open the password box straight away (used by `--preview`).
    pub fn show_prompt(&mut self, keyboard: bool) {
        self.prompt_open = true;
        self.keyboard = keyboard;
        self.focus_password = true;
        self.last_activity = Instant::now();
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.load_background(&ctx);
        self.poll_results(&ctx);
        if self.unlocked {
            return;
        }
        self.track_activity(&ctx);

        let rect = ctx.content_rect();
        let open = ctx.animate_bool_with_time(Id::unique("prompt"), self.prompt_open, 0.25);
        let kb = ctx.animate_bool_with_time(
            Id::unique("keyboard-shown"),
            self.prompt_open && self.keyboard,
            0.25,
        );
        self.paint_background(ui, rect, open);
        if open > 0.0 {
            self.prompt(&ctx, open, kb);
            if self.keyboard {
                self.on_screen_keyboard(&ctx, open);
            }
        }
        // Countdown after wrong passwords, and the fingerprint reader's news.
        ctx.request_repaint_after(Duration::from_millis(250));
    }

    fn load_background(&mut self, ctx: &egui::Context) {
        if let Some(img) = self.background_image.take() {
            let color =
                egui::ColorImage::from_rgba_unmultiplied([img.width, img.height], &img.rgba);
            self.background = Some(ctx.load_texture("background", color, TextureOptions::LINEAR));
        }
    }

    fn poll_results(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.checking {
            match rx.try_recv() {
                Ok(true) => {
                    self.checking = None;
                    self.unlock(ctx);
                    return;
                }
                Ok(false) | Err(TryRecvError::Disconnected) => {
                    self.checking = None;
                    self.throttle.record_failure(Instant::now());
                    self.rejected();
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        while let Some(rx) = &self.fingerprint {
            match rx.try_recv() {
                Ok(FingerEvent::Matched) => {
                    self.unlock(ctx);
                    return;
                }
                Ok(FingerEvent::NoMatch) => {
                    self.wake();
                    self.rejected();
                }
                Ok(FingerEvent::Unavailable) | Err(TryRecvError::Disconnected) => {
                    self.fingerprint = None;
                    self.finger_live = false;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        if let Some(rx) = &self.hello_pending {
            let outcome = match rx.try_recv() {
                Ok(o) => Some(o),
                Err(TryRecvError::Disconnected) => Some(HelloOutcome::Failed),
                Err(TryRecvError::Empty) => None,
            };
            if let Some(outcome) = outcome {
                self.hello_pending = None;
                self.last_activity = Instant::now();
                self.focus_password = true;
                match outcome {
                    HelloOutcome::Verified => self.unlock(ctx),
                    HelloOutcome::Cancelled => {}
                    HelloOutcome::Failed => self.shake_at = Some(Instant::now()),
                }
            }
        }
    }

    /// Wrong password or finger: shake, clear, try again.
    fn rejected(&mut self) {
        self.password.clear();
        self.shake_at = Some(Instant::now());
        self.focus_password = true;
    }

    fn wake(&mut self) {
        self.last_activity = Instant::now();
        if !self.prompt_open {
            self.prompt_open = true;
            self.focus_password = true;
        }
    }

    fn unlock(&mut self, ctx: &egui::Context) {
        self.unlocked = true;
        self.password.clear();
        (self.on_unlock)(ctx);
    }

    fn busy(&self) -> bool {
        self.checking.is_some() || self.hello_pending.is_some()
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
            if touched {
                self.keyboard = true;
            }
            if !self.prompt_open && self.throttle.remaining(now).is_none() {
                // Keep the key that woke the screen as the first character.
                self.password.push_str(&typed);
            }
            self.wake();
        }
        let idle = now.duration_since(self.last_activity) > self.timeout;
        let escape = ctx.input(|i| i.key_pressed(Key::Escape));
        if self.prompt_open && !self.busy() && (idle || escape) {
            self.prompt_open = false;
            self.password.clear();
            self.keyboard = false;
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
                let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
                painter.image(tex.id(), draw, uv, Color32::WHITE);
            }
            None => {
                painter.rect_filled(rect, 0.0, Color32::from_rgb(24, 26, 32));
            }
        }
        // A light veil, a little darker while the password box is up.
        painter.rect_filled(
            rect,
            0.0,
            Color32::from_black_alpha((35.0 + 55.0 * open) as u8),
        );
    }

    fn prompt(&mut self, ctx: &egui::Context, open: f32, kb: f32) {
        let shake = self
            .shake_at
            .map(|t| t.elapsed().as_secs_f32())
            .filter(|t| *t < 0.45)
            .map_or(0.0, |t| (t * 45.0).sin() * 12.0 * (1.0 - t / 0.45));
        if shake != 0.0 {
            ctx.request_repaint();
        }
        let rise = (1.0 - open) * 16.0;
        // Centred, or above the on-screen keyboard (about 300 points tall).
        let lift = -170.0 * kb;
        let blocked = self.throttle.remaining(Instant::now());
        let busy = self.busy();
        let rejected = shake != 0.0;
        let hello_button = !self.finger_live && self.hello.is_some();

        egui::Area::new(Id::unique("prompt-box"))
            .anchor(Align2::CENTER_CENTER, Vec2::new(shake, lift + rise))
            .movable(false)
            .show(ctx, |ui| {
                ui.multiply_opacity(open);
                let border = if rejected {
                    ERROR
                } else {
                    Color32::from_white_alpha(40)
                };
                egui::Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(16, 18, 24, 215))
                    .stroke(Stroke::new(1.0, border))
                    .corner_radius(CornerRadius::same(26))
                    .inner_margin(Margin {
                        left: 20,
                        right: 8,
                        top: 6,
                        bottom: 6,
                    })
                    .shadow(egui::Shadow {
                        offset: [0, 10],
                        blur: 36,
                        spread: 0,
                        color: Color32::from_black_alpha(100),
                    })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            let hint = match blocked {
                                Some(left) => format!("Try again in {}", fmt_duration(left)),
                                None => "Password".to_string(),
                            };
                            let edit = egui::TextEdit::singleline(&mut self.password)
                                .password(true)
                                .frame(egui::Frame::NONE)
                                .hint_text(
                                    RichText::new(hint).color(Color32::from_white_alpha(120)),
                                )
                                .font(FontId::proportional(18.0))
                                .text_color(Color32::WHITE)
                                .align(Align2::LEFT_CENTER)
                                .min_size(Vec2::new(0.0, 40.0))
                                .desired_width(FIELD_WIDTH - 60.0)
                                .interactive(blocked.is_none() && !busy);
                            let resp = ui.add(edit);
                            if self.focus_password && blocked.is_none() && !busy {
                                resp.request_focus();
                                self.focus_password = false;
                            }
                            if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                                self.submit();
                            }

                            // The fingerprint mark: a quiet sign the reader is
                            // listening, or a button for the Windows Hello prompt.
                            let (rect, resp) =
                                ui.allocate_exact_size(Vec2::splat(40.0), Sense::click());
                            if self.finger_live || hello_button {
                                let hovered = hello_button && resp.hovered() && !busy;
                                if hovered {
                                    ui.painter().circle_filled(
                                        rect.center(),
                                        19.0,
                                        Color32::from_white_alpha(20),
                                    );
                                }
                                let alpha = if busy || hovered { 240 } else { 150 };
                                let color = if rejected {
                                    ERROR
                                } else {
                                    Color32::from_white_alpha(alpha)
                                };
                                paint_fingerprint(ui.painter(), rect.center(), 11.0, color);
                                if hello_button
                                    && !busy
                                    && resp
                                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                                        .clicked()
                                {
                                    self.start_hello();
                                }
                            }
                        });
                    });
            });
    }

    fn submit(&mut self) {
        if self.password.is_empty()
            || self.busy()
            || self.throttle.remaining(Instant::now()).is_some()
        {
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
    }

    fn start_hello(&mut self) {
        if let Some(start) = &self.hello {
            self.hello_pending = Some(start());
        }
    }

    fn on_screen_keyboard(&mut self, ctx: &egui::Context, open: f32) {
        let rows: [&str; 4] = if self.shift {
            ["!@#$%^&*()", "QWERTYUIOP", "ASDFGHJKL", "ZXCVBNM-_."]
        } else {
            ["1234567890", "qwertyuiop", "asdfghjkl", "zxcvbnm-_."]
        };
        let busy = self.busy() || self.throttle.remaining(Instant::now()).is_some();
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
                    .fill(Color32::from_rgba_unmultiplied(16, 18, 24, 215))
                    .stroke(Stroke::new(1.0, Color32::from_white_alpha(28)))
                    .corner_radius(CornerRadius::same(18))
                    .inner_margin(Margin::same(14))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing = Vec2::splat(6.0);
                        for row in rows {
                            ui.horizontal(|ui| {
                                // Centre shorter rows.
                                let n = row.chars().count() as f32;
                                let full = 10.0 * key_size.x + 9.0 * 6.0;
                                ui.add_space((full - (n * key_size.x + (n - 1.0) * 6.0)) / 2.0);
                                for c in row.chars() {
                                    if key_button(ui, &c.to_string(), key_size, false).clicked() {
                                        pressed = Some(KbKey::Char(c));
                                    }
                                }
                            });
                        }
                        ui.horizontal(|ui| {
                            let h = key_size.y;
                            if key_button(ui, "Shift", Vec2::new(110.0, h), self.shift).clicked() {
                                pressed = Some(KbKey::Shift);
                            }
                            if key_button(ui, "Space", Vec2::new(250.0, h), false).clicked() {
                                pressed = Some(KbKey::Char(' '));
                            }
                            if key_button(ui, "Delete", Vec2::new(90.0, h), false).clicked() {
                                pressed = Some(KbKey::Backspace);
                            }
                            if key_button(ui, "Enter", Vec2::new(110.0, h), false).clicked() {
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
        Color32::from_white_alpha(70)
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

/// A simple fingerprint mark: nested arches, open at the bottom.
fn paint_fingerprint(painter: &egui::Painter, centre: Pos2, radius: f32, color: Color32) {
    let stroke = Stroke::new(1.6, color);
    for (i, r) in [0.25f32, 0.5, 0.75, 1.0].iter().enumerate() {
        let r = r * radius;
        // Inner arches are fuller; the outer ones open up more at the bottom.
        let gap = 0.35 + 0.15 * i as f32;
        let (start, end) = (PI * (0.5 + gap), PI * (2.5 - gap));
        let steps = 24;
        let points: Vec<Pos2> = (0..=steps)
            .map(|s| {
                let a = start + (end - start) * s as f32 / steps as f32;
                Pos2::new(centre.x + r * a.cos(), centre.y + 2.0 + r * 1.2 * a.sin())
            })
            .collect();
        painter.add(Shape::line(points, stroke));
    }
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
