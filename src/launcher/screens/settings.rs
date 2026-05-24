use crate::launcher::config::{LauncherSettings, VramBudget, resolve_default_tiles_dir};
use crate::launcher::style::*;
use crate::launcher::widgets::*;
use egui::{Id, Sense, Stroke, Ui, vec2};

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

    opt_row(ui, "06", "VRAM Budget", |ui| {
        let mut idx: u32 = match settings.vram_budget {
            VramBudget::Low => 0,
            VramBudget::Mid => 1,
            VramBudget::High => 2,
        };
        dropdown(
            ui,
            Id::new("vram_budget"),
            &["Low (≤4 GB)", "Mid (4–8 GB)", "High (8 GB+)"],
            &mut idx,
        );
        settings.vram_budget = match idx {
            0 => VramBudget::Low,
            2 => VramBudget::High,
            _ => VramBudget::Mid,
        };
    });

    opt_row(ui, "07", "Tiles directory", |ui| {
        // RTL: first added = rightmost
        if small_button(ui, "Browse…", SmallButtonVariant::Primary) {
            if let Some(dir) = rfd::FileDialog::new()
                .set_directory(&settings.tiles_dir)
                .pick_folder()
            {
                settings.tiles_dir = dir;
            }
        }
        ui.add_space(4.0);
        if copy_icon_button(ui) {
            ui.ctx()
                .copy_text(settings.tiles_dir.to_string_lossy().into_owned());
        }
        let default_dir = resolve_default_tiles_dir();
        if settings.tiles_dir != default_dir {
            ui.add_space(4.0);
            if small_button(ui, "Reset", SmallButtonVariant::Secondary) {
                settings.tiles_dir = default_dir;
            }
        }
        ui.add_space(8.0);
        // Show only the leaf folder name to avoid overlapping the row label.
        // Hovering reveals the full path via tooltip.
        let full_path = settings.tiles_dir.to_string_lossy().into_owned();
        let display = settings
            .tiles_dir
            .file_name()
            .map(|n| format!("…/{}", n.to_string_lossy()))
            .unwrap_or_else(|| full_path.clone());
        ui.label(
            egui::RichText::new(display)
                .font(mono(10.0))
                .color(TEXT_MUTED),
        )
        .on_hover_text(&full_path);
    });

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

// Kept in case this component is needed later.
#[allow(dead_code)]
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
