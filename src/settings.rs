//! `show-lock --settings`: set the password, shortcut and options.

use std::sync::mpsc::{self, Receiver};

use eframe::egui::{self, Color32, RichText, Vec2};

use crate::config::{Config, Hotkey, MIN_PASSWORD_LEN};
use crate::platform;

pub fn run() {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Show Lock settings")
            .with_inner_size([460.0, 650.0])
            .with_resizable(false)
            .with_icon(egui::IconData {
                rgba: crate::icon::padlock_rgba(64, [90, 140, 255]),
                width: 64,
                height: 64,
            }),
        ..Default::default()
    };
    let _ = eframe::run_native(
        "Show Lock settings",
        options,
        Box::new(|cc| {
            crate::style::setup(&cc.egui_ctx);
            Ok(Box::new(Settings::new()))
        }),
    );
}

struct Settings {
    config: Config,
    new_password: String,
    confirm: String,
    startup: bool,
    hello_status: HelloStatus,
    hello_rx: Option<Receiver<bool>>,
    status: Option<(String, bool)>,
    frames: u32,
}

enum HelloStatus {
    Checking,
    Available,
    Unavailable,
}

impl Settings {
    fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(platform::hello_available());
        });
        Self {
            config: Config::load(),
            new_password: String::new(),
            confirm: String::new(),
            startup: platform::startup_enabled(),
            hello_status: HelloStatus::Checking,
            hello_rx: Some(rx),
            status: None,
            frames: 0,
        }
    }

    fn save(&mut self) -> Result<(), String> {
        let mut config = self.config.clone();
        if !self.new_password.is_empty() || !config.has_password() {
            if self.new_password != self.confirm {
                return Err("The two passwords don't match.".into());
            }
            config.set_password(&self.new_password)?;
        }
        config.hotkey.validate()?;
        config.save()?;
        platform::set_startup(self.startup)?;
        self.config = config;
        self.new_password.clear();
        self.confirm.clear();
        Ok(())
    }
}

impl eframe::App for Settings {
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let Some(rx) = &self.hello_rx {
            if let Ok(ok) = rx.try_recv() {
                self.hello_status = if ok {
                    HelloStatus::Available
                } else {
                    HelloStatus::Unavailable
                };
                self.hello_rx = None;
            } else {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(200));
            }
        }

        egui::Frame::central_panel(ui.style()).inner_margin(24).show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
            ui.heading(RichText::new("Show Lock").size(24.0));
            ui.label(RichText::new("Locks this PC without blanking your other screens.").color(Color32::GRAY));
            ui.add_space(12.0);

            section(ui, "Password", |ui| {
                if self.config.has_password() {
                    ui.label(RichText::new("A password is set. Fill these in only to change it.").color(Color32::GRAY));
                } else {
                    ui.label(RichText::new("Choose the password that unlocks the lock screen. It is separate from your Windows password.").color(Color32::GRAY));
                }
                password_field(ui, &mut self.new_password, "New password");
                password_field(ui, &mut self.confirm, "Type it again");
                ui.label(RichText::new(format!("At least {MIN_PASSWORD_LEN} characters.")).small().color(Color32::GRAY));
            });

            section(ui, "Fingerprint", |ui| {
                ui.checkbox(&mut self.config.windows_hello, "Also unlock with my fingerprint (Windows Hello)");
                let (text, color) = match self.hello_status {
                    HelloStatus::Checking => ("Checking this PC…", Color32::GRAY),
                    HelloStatus::Available => ("Available: fingerprint, face or Windows PIN.", Color32::from_rgb(120, 200, 140)),
                    HelloStatus::Unavailable => ("Not set up on this PC (Windows Settings > Accounts > Sign-in options).", Color32::from_rgb(230, 180, 90)),
                };
                ui.label(RichText::new(text).small().color(color));
            });

            section(ui, "Lock shortcut", |ui| {
                let hk = &mut self.config.hotkey;
                ui.horizontal(|ui| {
                    ui.checkbox(&mut hk.ctrl, "Ctrl");
                    ui.checkbox(&mut hk.alt, "Alt");
                    ui.checkbox(&mut hk.shift, "Shift");
                    ui.checkbox(&mut hk.win, "Win");
                    egui::ComboBox::from_id_salt("key").width(60.0).selected_text(hk.key.to_string()).show_ui(ui, |ui| {
                        for c in Hotkey::KEYS.chars() {
                            ui.selectable_value(&mut hk.key, c, c.to_string());
                        }
                    });
                });
                match hk.validate() {
                    Ok(()) => ui.label(RichText::new(format!("Press {} to lock.", hk.label())).small().color(Color32::GRAY)),
                    Err(e) => ui.label(RichText::new(e).small().color(Color32::from_rgb(255, 120, 120))),
                };
            });

            section(ui, "Options", |ui| {
                ui.horizontal(|ui| {
                    ui.label("Hide the password box after");
                    ui.add(egui::DragValue::new(&mut self.config.prompt_timeout_secs).range(5..=600).suffix(" s"));
                });
                ui.checkbox(&mut self.startup, "Start Show Lock when I sign in to Windows");
            });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let save = egui::Button::new(RichText::new("Save").color(Color32::WHITE)).fill(Color32::from_rgb(70, 110, 210)).min_size(Vec2::new(110.0, 34.0));
                if ui.add(save).clicked() {
                    self.status = Some(match self.save() {
                        Ok(()) => ("Saved.".into(), false),
                        Err(e) => (e, true),
                    });
                }
                if ui.add(egui::Button::new("Preview lock screen").min_size(Vec2::new(0.0, 34.0))).on_hover_text("Opens the lock screen in a window without locking anything.").clicked() {
                    platform::spawn_self("--preview");
                }
            });
            if let Some((msg, err)) = &self.status {
                ui.label(RichText::new(msg).color(if *err { Color32::from_rgb(255, 120, 120) } else { Color32::from_rgb(120, 200, 140) }));
            }
        });
        crate::preview::dev_screenshot(ui.ctx(), &mut self.frames);
    }
}

fn section(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::group(ui.style())
        .inner_margin(12)
        .corner_radius(10)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).strong().size(15.0));
            add(ui);
        });
}

fn password_field(ui: &mut egui::Ui, value: &mut String, hint: &str) {
    ui.add(
        egui::TextEdit::singleline(value)
            .password(true)
            .hint_text(hint)
            .margin(egui::Margin::symmetric(8, 6))
            .desired_width(f32::INFINITY),
    );
}
