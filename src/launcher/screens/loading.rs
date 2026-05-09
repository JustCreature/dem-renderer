use crate::launcher::style::*;
use egui::{Rect, Ui, vec2};

pub fn show(ui: &mut Ui, progress: f32, status: &str) {
    ui.label(
        egui::RichText::new("Loading terrain data…")
            .font(prop(18.0))
            .color(TEXT_PRIMARY),
    );
    ui.add_space(14.0);

    let bar_h = 4.0_f32;
    let (bar_rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), bar_h), egui::Sense::hover());
    ui.painter()
        .rect_filled(bar_rect, egui::CornerRadius::same(0), HAIRLINE);
    let fill_w = (bar_rect.width() * progress.clamp(0.0, 1.0)).max(2.0);
    let fill_rect = Rect::from_min_size(bar_rect.min, vec2(fill_w, bar_h));
    ui.painter()
        .rect_filled(fill_rect, egui::CornerRadius::same(0), TEXT_PRIMARY);

    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(status)
                .font(mono(10.0))
                .color(TEXT_SECONDARY),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!("{:.0}%", progress * 100.0))
                    .font(mono(10.0))
                    .color(TEXT_SECONDARY),
            );
        });
    });

    ui.ctx().request_repaint();
}
