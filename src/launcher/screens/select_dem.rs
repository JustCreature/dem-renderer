use crate::launcher::config::SelectedView;
use crate::launcher::style::*;
use crate::launcher::widgets::*;
use egui::{Color32, Id, Sense, Stroke, Ui, vec2};
use std::path::Path;

#[derive(Default)]
pub struct SelectDemAnim {
    pub choice: [f32; 2],
}

pub enum SelectDemEvent {
    ChooseFiles,
    DemoView,
    Reset,
}

/// Renders only the choice cards. Header, footer, and back navigation are owned by mod.rs.
/// `tile_display` is the filename portion of the current tile path (shown when `tile_is_custom`).
pub fn show(
    ui: &mut Ui,
    anim: &mut SelectDemAnim,
    modal_open: &mut bool,
    tile_display: &str,
    tile_is_custom: bool,
    selected_view: &SelectedView,
    tiles_dir: &Path,
) -> Option<SelectDemEvent> {
    let mut event = None;

    // Populate free-space cache so mod.rs footer can display it.
    // Cache key includes the tiles_dir so it refreshes when the user changes it.
    let cache_key = Id::new(("free_space_cache", tiles_dir.to_string_lossy().as_ref()));
    ui.ctx().data_mut(|d| {
        if d.get_temp::<String>(cache_key).is_none() {
            d.insert_temp(cache_key, get_free_space(tiles_dir));
        }
    });

    if choice_item(
        ui,
        "A",
        "Choose files…",
        "Open a file browser and pick local DEM tiles.\nSupports .tif (GeoTIFF).",
        "LOCAL · ANY SIZE",
        *selected_view == SelectedView::CustomFile,
        &mut anim.choice[0],
    ) {
        event = Some(SelectDemEvent::ChooseFiles);
    }

    if tile_is_custom {
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(format!("▸  {tile_display}"))
                    .font(mono(10.5))
                    .color(TEXT_SECONDARY),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        egui::Label::new(
                            egui::RichText::new("Reset to default")
                                .font(mono(9.5))
                                .color(TEXT_MUTED),
                        )
                        .sense(egui::Sense::click()),
                    )
                    .clicked()
                {
                    event = Some(SelectDemEvent::Reset);
                }
            });
        });
        ui.add_space(4.0);
    }

    if choice_item(
        ui,
        "B",
        "Recommended demo view",
        "5m Austria BEV DEM + two 1m Tirol tiles (Innsbruck area).\nBest way to test the full multi-resolution renderer.",
        "REMOTE · ~45 GB · DOWNLOAD ON START",
        *selected_view == SelectedView::DemoView,
        &mut anim.choice[1],
    ) {
        *modal_open = true;
    }

    // Bottom hairline closes the last card visually
    let (_, p) = ui.allocate_painter(vec2(ui.available_width(), 1.0), Sense::hover());
    p.line_segment(
        [p.clip_rect().left_top(), p.clip_rect().right_top()],
        Stroke::new(1.0, HAIRLINE),
    );

    if *modal_open && let Some(e) = show_download_modal(ui, modal_open, tiles_dir, cache_key) {
        event = Some(e);
    }

    event
}

fn show_download_modal(
    ui: &mut Ui,
    modal_open: &mut bool,
    tiles_dir: &Path,
    cache_key: Id,
) -> Option<SelectDemEvent> {
    let screen = ui.ctx().content_rect();

    let scrim_painter = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        Id::new("modal_scrim"),
    ));
    scrim_painter.rect_filled(screen, egui::CornerRadius::same(0), SCRIM);

    let modal_w = 460.0_f32;
    let mut event = None;
    let id = Id::new("download_modal");

    let free_str: String = ui.ctx().data(|d| {
        d.get_temp::<String>(cache_key)
            .unwrap_or_else(|| "—".to_string())
    });

    egui::Area::new(id)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .order(egui::Order::Tooltip)
        .show(ui.ctx(), |ui| {
            egui::Frame::NONE
                .fill(Color32::from_rgba_premultiplied(18, 18, 20, 245))
                .stroke(egui::Stroke::new(1.0, PANEL_BORDER))
                .inner_margin(egui::Margin::symmetric(30, 28))
                .show(ui, |ui| {
                    ui.set_width(modal_w - 60.0);

                    ui.horizontal(|ui| {
                        let (dot_r, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
                        ui.painter().circle_filled(
                            dot_r.center(),
                            3.0,
                            Color32::from_rgb(217, 156, 122),
                        );
                        ui.label(
                            egui::RichText::new("Confirm Download")
                                .font(mono(10.0))
                                .color(TEXT_MUTED),
                        );
                    });
                    ui.add_space(12.0);

                    ui.label(
                        egui::RichText::new("Download recommended demo dataset?")
                            .font(prop(22.0))
                            .color(TEXT_PRIMARY),
                    );
                    ui.add_space(10.0);

                    ui.label(
                        egui::RichText::new(
                            "~45 GB will be downloaded to the directory shown below. \
                         The download resumes if interrupted and only runs once — \
                         subsequent launches reuse the cached tiles.",
                        )
                        .font(prop(13.5))
                        .color(TEXT_SECONDARY),
                    );
                    ui.add_space(14.0);

                    hairline_rule(ui);
                    ui.add_space(10.0);
                    ui.columns(2, |cols| {
                        stat_cell(&mut cols[0], "Size", "~45 GB");
                        stat_cell_with_tip(
                            &mut cols[1],
                            "Tiles",
                            "28 tiles",
                            "25 Copernicus GLO-30 (5×5° grid, 30m)\n\
                             1 whole-Austria BEV DEM (5m)\n\
                             2 Tirol 1m tiles — Innsbruck area\n\
                             + adjacent Salzburg (eastern) area",
                        );
                        stat_cell(&mut cols[0], "Region", "Austria · 46–50°N");
                        stat_cell(&mut cols[1], "Free space", &free_str);
                    });
                    ui.add_space(10.0);
                    hairline_rule(ui);
                    ui.add_space(12.0);

                    let path_str = tiles_dir.join("big_size").to_string_lossy().into_owned();
                    text_area(ui, Id::new("download_path"), &path_str, false);
                    ui.add_space(16.0);

                    ui.horizontal(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if main_button(ui, "DOWNLOAD & START", ButtonVariant::Primary) {
                                *modal_open = false;
                                event = Some(SelectDemEvent::DemoView);
                            }
                            ui.add_space(10.0);
                            if main_button(ui, "CANCEL", ButtonVariant::Secondary) {
                                *modal_open = false;
                            }
                        });
                    });
                });
        });

    event
}

fn stat_cell(ui: &mut Ui, key: &str, val: &str) {
    ui.label(egui::RichText::new(key).font(mono(9.5)).color(TEXT_MUTED));
    ui.label(
        egui::RichText::new(val)
            .font(mono(12.0))
            .color(TEXT_PRIMARY),
    );
    ui.add_space(6.0);
}

fn stat_cell_with_tip(ui: &mut Ui, key: &str, val: &str, tip: &str) {
    ui.label(egui::RichText::new(key).font(mono(9.5)).color(TEXT_MUTED));
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(val)
                .font(mono(12.0))
                .color(TEXT_PRIMARY),
        );
        ui.add_space(4.0);
        info_tooltip_button(ui, Id::new("tiles_tip"), tip);
    });
    ui.add_space(6.0);
}

fn get_free_space(tiles_dir: &Path) -> String {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let dir_str = tiles_dir.to_string_lossy().to_string();
    let mut best: Option<(usize, u64)> = None;
    for disk in disks.list() {
        let mount = disk.mount_point().to_string_lossy().to_string();
        if dir_str.starts_with(&mount) {
            let len = mount.len();
            if len > best.map(|(l, _)| l).unwrap_or(0) {
                best = Some((len, disk.available_space()));
            }
        }
    }
    match best {
        Some((_, free)) => {
            let gb = free / 1_073_741_824;
            if gb > 0 {
                format!("{gb} GB")
            } else {
                format!("{} MB", free / 1_048_576)
            }
        }
        None => "unknown".to_string(),
    }
}
