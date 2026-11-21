//! Shared look: Windows' own Segoe UI fonts when available, dark theme,
//! gradients.

use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Pos2, Rect,
};

/// Thin font used for the big clock.
pub fn light_family() -> FontFamily {
    FontFamily::Name("light".into())
}

/// Fonts and dark visuals for every window.
pub fn setup(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let mut light = Vec::new();
    // Read from Windows' font folder at runtime, so nothing is bundled.
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let load = |file: &str| std::fs::read(format!(r"{windir}\Fonts\{file}")).ok();
    if let Some(bytes) = load("segoeui.ttf") {
        fonts
            .font_data
            .insert("segoe".into(), Arc::new(FontData::from_owned(bytes)));
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, "segoe".into());
    }
    if let Some(bytes) = load("segoeuil.ttf") {
        fonts
            .font_data
            .insert("segoe-light".into(), Arc::new(FontData::from_owned(bytes)));
        light.push("segoe-light".to_string());
    }
    // Fall back to the normal proportional fonts (and their symbol fallbacks).
    light.extend(
        fonts
            .families
            .get(&FontFamily::Proportional)
            .cloned()
            .unwrap_or_default(),
    );
    fonts.families.insert(light_family(), light);
    ctx.set_fonts(fonts);

    let mut visuals = egui::Visuals::dark();
    let radius = CornerRadius::same(10);
    visuals.widgets.noninteractive.corner_radius = radius;
    visuals.widgets.inactive.corner_radius = radius;
    visuals.widgets.hovered.corner_radius = radius;
    visuals.widgets.active.corner_radius = radius;
    visuals.widgets.open.corner_radius = radius;
    visuals.extreme_bg_color = Color32::from_rgba_unmultiplied(255, 255, 255, 14);
    visuals.selection.bg_fill = Color32::from_rgb(70, 110, 210);
    ctx.set_visuals(visuals);
}

/// Fill `rect` with a top-to-bottom gradient.
pub fn paint_gradient(painter: &egui::Painter, rect: Rect, top: Color32, bottom: Color32) {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(Pos2::new(rect.right(), rect.bottom()), bottom);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(mesh);
}
