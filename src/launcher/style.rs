use egui::Color32;

pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(244, 240, 232);
pub const TEXT_SECONDARY: Color32 = Color32::from_rgba_premultiplied(185, 182, 176, 209);
pub const TEXT_MUTED: Color32 = Color32::from_rgba_premultiplied(90, 89, 86, 102);
pub const TEXT_MUTED_55: Color32 = Color32::from_rgba_premultiplied(125, 124, 120, 140);
pub const PANEL_BG: Color32 = Color32::from_rgba_premultiplied(5, 5, 6, 107);
pub const PANEL_BORDER: Color32 = Color32::from_rgba_premultiplied(22, 22, 20, 26);
pub const HAIRLINE: Color32 = Color32::from_rgba_premultiplied(17, 17, 15, 20);
pub const SEG_ACTIVE: Color32 = Color32::from_rgba_premultiplied(40, 40, 38, 46);
pub const SEG_BORDER: Color32 = Color32::from_rgba_premultiplied(40, 40, 38, 46);
pub const GREEN_DOT: Color32 = Color32::from_rgb(108, 171, 122);
pub const DANGER: Color32 = Color32::from_rgb(217, 156, 122);
pub const SCRIM: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 140);
pub const TOOLTIP_BG: Color32 = Color32::from_rgba_premultiplied(18, 18, 19, 245);

pub fn mono(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Monospace)
}

pub fn prop(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Proportional)
}

pub fn prop_medium(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("SpaceGrotesk-Medium".into()))
}
