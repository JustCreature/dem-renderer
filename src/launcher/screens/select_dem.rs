use crate::launcher::config::SelectedView;
use crate::launcher::style::*;
use crate::launcher::widgets::*;
use egui::{Color32, Id, Sense, Stroke, Ui, pos2, vec2};

use crate::consts::TILES_BIG_PATH;

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
) -> Option<SelectDemEvent> {
    let mut event = None;

    // Populate free-space cache so mod.rs footer can display it
    ui.ctx().data_mut(|d| {
        if d.get_temp::<String>(Id::new("free_space_cache")).is_none() {
            d.insert_temp(Id::new("free_space_cache"), get_free_space());
        }
    });

    if choice_item(
        ui,
        "A",
        "Choose files…",
        "Open a file browser and pick local DEM tiles.\nSupports .tif (GeoTIFF) · .hgt (SRTM) · .asc (ESRI ASCII Grid).",
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

    if *modal_open {
        if let Some(e) = show_download_modal(ui, modal_open) {
            event = Some(e);
        }
    }

    event
}

fn show_download_modal(ui: &mut Ui, modal_open: &mut bool) -> Option<SelectDemEvent> {
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
        d.get_temp::<String>(Id::new("free_space_cache"))
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
                            "~45 GB will be downloaded to the tiles/big_size/ directory. \
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

                    // Path styled as a dark code block
                    let path_galley = ui.ctx().fonts_mut(|f| {
                        f.layout_no_wrap(TILES_BIG_PATH.to_string(), mono(11.0), TEXT_SECONDARY)
                    });
                    let path_h = path_galley.size().y + 14.0;
                    let (path_resp, path_painter) =
                        ui.allocate_painter(vec2(ui.available_width(), path_h), Sense::hover());
                    let pr = path_resp.rect;
                    path_painter.rect_filled(
                        pr,
                        egui::CornerRadius::same(2),
                        Color32::from_rgba_premultiplied(6, 6, 8, 220),
                    );
                    path_painter.rect_stroke(
                        pr,
                        egui::CornerRadius::same(2),
                        Stroke::new(1.0, HAIRLINE),
                        egui::StrokeKind::Inside,
                    );
                    path_painter.galley(
                        pos2(pr.min.x + 10.0, pr.center().y - path_galley.size().y * 0.5),
                        path_galley,
                        TEXT_SECONDARY,
                    );
                    ui.add_space(16.0);

                    ui.horizontal(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let primary = egui::Button::new(
                                egui::RichText::new("DOWNLOAD & START")
                                    .font(prop_medium(13.0))
                                    .color(Color32::from_rgb(17, 17, 17)),
                            )
                            .fill(Color32::from_rgba_premultiplied(232, 228, 220, 235))
                            .stroke(Stroke::new(1.0, TEXT_PRIMARY))
                            .min_size(vec2(160.0, 38.0))
                            .corner_radius(egui::CornerRadius::same(0));

                            if ui.add(primary).clicked() {
                                *modal_open = false;
                                event = Some(SelectDemEvent::DemoView);
                            }
                            ui.add_space(10.0);

                            let cancel = egui::Button::new(
                                egui::RichText::new("CANCEL")
                                    .font(prop(13.0))
                                    .color(TEXT_SECONDARY),
                            )
                            .fill(Color32::TRANSPARENT)
                            .stroke(Stroke::new(1.0, TEXT_MUTED_55))
                            .min_size(vec2(90.0, 38.0))
                            .corner_radius(egui::CornerRadius::same(0));

                            if ui.add(cancel).clicked() {
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

fn get_free_space() -> String {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let cwd = std::env::current_dir().ok().unwrap_or_default();
    let cwd_str = cwd.to_string_lossy().to_string();
    let mut best: Option<(usize, u64)> = None;
    for disk in disks.list() {
        let mount = disk.mount_point().to_string_lossy().to_string();
        if cwd_str.starts_with(&mount) {
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
