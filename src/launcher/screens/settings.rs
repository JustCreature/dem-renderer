use crate::launcher::config::LauncherSettings;
use crate::launcher::style::*;
use crate::launcher::widgets::*;
use egui::{vec2, Id, Sense, Stroke, Ui};

/// Renders only the six option rows. Header and footer are owned by mod.rs.
pub fn show(ui: &mut Ui, settings: &mut LauncherSettings) {
    opt_row(ui, "01", "Overall Quality", |ui| {
        segmented_control(
            ui,
            Id::new("quality"),
            &["Ultra", "High", "Mid", "Low"],
            &mut settings.vat_mode,
        );
    });

    opt_row(ui, "02", "Level of Detail", |ui| {
        segmented_control(
            ui,
            Id::new("lod"),
            &["Ultra", "High", "Mid", "Low"],
            &mut settings.lod_mode,
        );
    });

    opt_row(ui, "03", "Shadows", |ui| {
        let mut idx = if settings.shadows_enabled { 1 } else { 0 };
        if segmented_control(ui, Id::new("shadows"), &["Off", "On"], &mut idx) {
            settings.shadows_enabled = idx == 1;
        }
    });

    opt_row(ui, "04", "Fog", |ui| {
        let mut idx = if settings.fog_enabled { 1 } else { 0 };
        if segmented_control(ui, Id::new("fog"), &["Off", "On"], &mut idx) {
            settings.fog_enabled = idx == 1;
        }
    });

    opt_row(ui, "05", "Ambient Occlusion", |ui| {
        dropdown(
            ui,
            Id::new("ao"),
            &["Off", "SSAO×8", "SSAO×16", "HBAO×4", "HBAO×8", "True Hemi."],
            &mut settings.ao_mode,
        );
    });

    opt_row_with_info(
        ui, "06", "Use tiles refinement",
        "If a higher-resolution DEM is available for this region, it will be used to refine the rendered world.",
        |ui| {
            styled_checkbox(ui, &mut settings.tiles_refinement);
        },
    );

    // Bottom hairline closes the last row visually
    let (_, p) = ui.allocate_painter(vec2(ui.available_width(), 1.0), Sense::hover());
    p.line_segment(
        [p.clip_rect().left_top(), p.clip_rect().right_top()],
        Stroke::new(1.0, HAIRLINE),
    );
}

fn opt_row(ui: &mut Ui, num: &str, label: &str, control: impl FnOnce(&mut Ui)) {
    ui.add_space(4.0);
    let (_, p) = ui.allocate_painter(vec2(ui.available_width(), 1.0), Sense::hover());
    p.line_segment(
        [p.clip_rect().left_top(), p.clip_rect().right_top()],
        Stroke::new(1.0, HAIRLINE),
    );

    ui.horizontal(|ui| {
        ui.set_min_height(28.0);
        ui.label(egui::RichText::new(num).font(mono(10.0)).color(TEXT_MUTED));
        ui.add_space(10.0);
        ui.label(
            egui::RichText::new(label)
                .font(prop(15.0))
                .color(TEXT_SECONDARY),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            control(ui);
        });
    });
    ui.add_space(4.0);
}

fn opt_row_with_info(
    ui: &mut Ui,
    num: &str,
    label: &str,
    tooltip: &str,
    control: impl FnOnce(&mut Ui),
) {
    ui.add_space(4.0);
    let (_, p) = ui.allocate_painter(vec2(ui.available_width(), 1.0), Sense::hover());
    p.line_segment(
        [p.clip_rect().left_top(), p.clip_rect().right_top()],
        Stroke::new(1.0, HAIRLINE),
    );

    ui.horizontal(|ui| {
        ui.set_min_height(28.0);
        ui.label(egui::RichText::new(num).font(mono(10.0)).color(TEXT_MUTED));
        ui.add_space(10.0);
        ui.label(
            egui::RichText::new(label)
                .font(prop(15.0))
                .color(TEXT_SECONDARY),
        );
        info_tooltip_button(ui, Id::new("info_tiles_ref"), tooltip);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            control(ui);
        });
    });
    ui.add_space(4.0);
}
