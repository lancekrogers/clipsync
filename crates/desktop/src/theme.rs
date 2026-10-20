use eframe::egui::{self, Color32, RichText};
pub const BG: Color32 = Color32::from_rgb(17, 22, 28);
pub const SURFACE: Color32 = Color32::from_rgb(26, 34, 44);
pub const TEXT: Color32 = Color32::from_rgb(237, 242, 248);
pub const MUTED: Color32 = Color32::from_rgb(159, 174, 191);
pub const BLUE: Color32 = Color32::from_rgb(105, 191, 255);
pub const TEAL: Color32 = Color32::from_rgb(96, 212, 188);
pub const SELECTED: Color32 = Color32::from_rgb(33, 54, 75);
pub fn install(ctx: &egui::Context) {
    let mut style = (*ctx.global_style()).clone();
    style.spacing.item_spacing = egui::vec2(12.0, 10.0);
    style.spacing.button_padding = egui::vec2(18.0, 12.0);
    for (kind, size) in [
        (egui::TextStyle::Body, 18.0),
        (egui::TextStyle::Button, 18.0),
        (egui::TextStyle::Small, 15.0),
        (egui::TextStyle::Heading, 28.0),
    ] {
        style
            .text_styles
            .insert(kind, egui::FontId::proportional(size));
    }
    let mut v = egui::Visuals::dark();
    v.override_text_color = Some(TEXT);
    v.weak_text_color = Some(MUTED);
    v.panel_fill = BG;
    v.window_fill = SURFACE;
    v.extreme_bg_color = BG;
    v.selection.bg_fill = SELECTED;
    v.selection.stroke = egui::Stroke::new(1.5, BLUE);
    v.widgets.inactive.bg_fill = Color32::from_rgb(39, 50, 64);
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(39, 50, 64);
    v.widgets.inactive.bg_stroke = egui::Stroke::NONE;
    v.widgets.hovered.bg_fill = Color32::from_rgb(48, 66, 84);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(48, 66, 84);
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, BLUE);
    v.widgets.active.bg_fill = SELECTED;
    for widget in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
    ] {
        widget.corner_radius = egui::CornerRadius::same(12);
    }
    style.visuals = v;
    ctx.set_global_style(style);
    // Read the system font locally. Never bundle Apple's font in distributed assets.
    #[cfg(target_os = "macos")]
    if let Ok(bytes) = std::fs::read("/System/Library/Fonts/SFNS.ttf") {
        let mut fonts = egui::FontDefinitions::default();
        fonts
            .font_data
            .insert("system".into(), egui::FontData::from_owned(bytes).into());
        fonts
            .families
            .get_mut(&egui::FontFamily::Proportional)
            .unwrap()
            .insert(0, "system".into());
        ctx.set_fonts(fonts);
    }
}
pub fn card<R>(
    ui: &mut egui::Ui,
    contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let width = ui.available_width();
    egui::Frame::new()
        .fill(SURFACE)
        .corner_radius(18)
        .inner_margin(26)
        .show(ui, |ui| {
            ui.set_min_width((width - 52.0).max(100.0));
            contents(ui)
        })
}
pub fn primary(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).color(Color32::from_rgb(10, 30, 47)))
            .fill(BLUE)
            .stroke(egui::Stroke::NONE),
    )
}
