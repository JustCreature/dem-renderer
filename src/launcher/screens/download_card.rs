use std::f32::consts::{FRAC_PI_2, TAU};
use std::time::Duration;

use egui::epaint::{PathShape, PathStroke};
use egui::{Color32, Id, Margin, Order, Pos2, Sense, Shape, Stroke, Vec2};

use crate::launcher::downloader::DownloadProgress;
use crate::launcher::style::*;

pub enum DownloadCardEvent {
    Cancel,
    Dismiss,
}

pub fn show(ctx: &egui::Context, progress: &DownloadProgress) -> Option<DownloadCardEvent> {
    let mut event = None;

    egui::Area::new(Id::new("dl_card"))
        .anchor(egui::Align2::LEFT_BOTTOM, Vec2::new(24.0, -80.0))
        .order(Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(Color32::from_rgba_premultiplied(14, 14, 16, 210))
                .stroke(Stroke::new(
                    1.0,
                    Color32::from_rgba_premultiplied(40, 40, 38, 60),
                ))
                .inner_margin(Margin {
                    left: 20,
                    right: 22,
                    top: 18,
                    bottom: 18,
                })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(16.0, 0.0);

                        // ── Ring ──────────────────────────────────────
                        let (ring_resp, ring_painter) =
                            ui.allocate_painter(Vec2::new(72.0, 72.0), Sense::hover());
                        let center = ring_resp.rect.center();

                        const RING_R: f32 = 28.0;
                        const RING_W: f32 = 4.5;
                        const RING_PTS: usize = 128;

                        let frac = overall_frac(progress);

                        // Track ring — full closed circle as a PathShape so both
                        // track and arc go through the same tessellator path, avoiding
                        // the "double ring" mismatch between circle_stroke and PathShape::line.
                        {
                            let pts: Vec<Pos2> = (0..RING_PTS)
                                .map(|i| {
                                    let a = i as f32 / RING_PTS as f32 * TAU;
                                    egui::pos2(
                                        center.x + RING_R * a.cos(),
                                        center.y + RING_R * a.sin(),
                                    )
                                })
                                .collect();
                            ring_painter.add(Shape::Path(PathShape {
                                points: pts,
                                closed: true,
                                fill: Color32::TRANSPARENT,
                                stroke: PathStroke::new(RING_W, TEXT_MUTED_55),
                            }));
                        }

                        // Progress arc
                        if frac > 0.001 {
                            let start = -FRAC_PI_2;
                            let sweep = frac * TAU;
                            let n = ((RING_PTS as f32 * frac).ceil() as usize).max(2);
                            let pts: Vec<Pos2> = (0..n)
                                .map(|k| {
                                    let t = k as f32 / (n - 1) as f32;
                                    let a = start + t * sweep;
                                    egui::pos2(
                                        center.x + RING_R * a.cos(),
                                        center.y + RING_R * a.sin(),
                                    )
                                })
                                .collect();
                            ring_painter.add(Shape::Path(PathShape::line(
                                pts,
                                Stroke::new(RING_W, GREEN_DOT),
                            )));
                        }

                        // Center label
                        let (center_text, center_color) = if progress.is_complete {
                            ("✓".to_string(), GREEN_DOT)
                        } else {
                            (format!("{}%", (frac * 100.0) as u32), TEXT_PRIMARY)
                        };
                        ring_painter.text(
                            center,
                            egui::Align2::CENTER_CENTER,
                            center_text,
                            mono(11.0),
                            center_color,
                        );

                        // ── Body ──────────────────────────────────────
                        ui.vertical(|ui| {
                            ui.set_width(285.0);
                            ui.spacing_mut().item_spacing = Vec2::new(0.0, 5.0);

                            // Header row
                            ui.horizontal(|ui| {
                                let label = if progress.is_complete {
                                    "Complete"
                                } else if progress.bytes_total == 0 {
                                    "Checking"
                                } else {
                                    "Downloading"
                                };
                                ui.label(
                                    egui::RichText::new(label)
                                        .font(mono(10.0))
                                        .color(TEXT_MUTED),
                                );
                                if !progress.is_complete && progress.error.is_none() {
                                    ui.add_space(6.0);
                                    let time = ctx.input(|i| i.time) as f32;
                                    let alpha = (time * TAU).sin() * 0.5 + 0.5;
                                    let dot_color = Color32::from_rgba_unmultiplied(
                                        GREEN_DOT.r(),
                                        GREEN_DOT.g(),
                                        GREEN_DOT.b(),
                                        (alpha * 210.0) as u8,
                                    );
                                    let (dot_resp, dot_p) =
                                        ui.allocate_painter(Vec2::new(10.0, 10.0), Sense::hover());
                                    dot_p.circle_filled(dot_resp.rect.center(), 3.0, dot_color);
                                    ctx.request_repaint_after(Duration::from_millis(200));
                                }
                            });

                            // Filename / completion label
                            let filename = if progress.is_complete {
                                "All 28 files ready".to_string()
                            } else {
                                progress.display_name.clone()
                            };
                            ui.label(
                                egui::RichText::new(filename)
                                    .font(mono(11.0))
                                    .color(TEXT_PRIMARY),
                            );

                            // Speed: EMA over successive frames stored in egui temp data.
                            // State: (prev_bytes: u64, prev_time: f64, smoothed_bps: f64)
                            let speed_id = Id::new("dl_speed");
                            let now = ctx.input(|i| i.time);
                            let smoothed_bps: f64 = if progress.bytes_total > 0
                                && !progress.is_complete
                            {
                                let (prev_bytes, prev_time, prev_speed): (u64, f64, f64) = ctx
                                    .data(|d| {
                                        d.get_temp(speed_id).unwrap_or((0u64, 0.0f64, 0.0f64))
                                    });
                                let dt = now - prev_time;
                                if dt > 0.05 && progress.bytes_done >= prev_bytes {
                                    let instant = (progress.bytes_done - prev_bytes) as f64 / dt;
                                    let clamped = if prev_speed > 0.0 {
                                        instant.min(prev_speed * 4.0)
                                    } else {
                                        instant
                                    };
                                    // EMA α=0.10 — ~10 s to fully track a new sustained rate
                                    let s = if prev_speed > 0.0 {
                                        0.10 * clamped + 0.90 * prev_speed
                                    } else {
                                        clamped
                                    };
                                    ctx.data_mut(|d| {
                                        d.insert_temp(speed_id, (progress.bytes_done, now, s))
                                    });
                                    s
                                } else {
                                    prev_speed
                                }
                            } else {
                                ctx.data_mut(|d| d.remove::<(u64, f64, f64)>(speed_id));
                                0.0
                            };

                            let speed_str = if smoothed_bps >= 1_000_000.0 {
                                format!("  ·  {:.1} MB/s", smoothed_bps / 1_000_000.0)
                            } else if smoothed_bps >= 1_000.0 {
                                format!("  ·  {:.0} KB/s", smoothed_bps / 1_000.0)
                            } else {
                                String::new()
                            };

                            // Stats
                            let stats_text = if progress.is_complete {
                                "~45 GB · tiles/".to_string()
                            } else if progress.bytes_total == 0 {
                                format!(
                                    "{} / {} checked",
                                    progress.file_index + 1,
                                    progress.total_files,
                                )
                            } else if progress.current_file_size > 0 {
                                format!(
                                    "{} / {} files  ·  {} / {}{}",
                                    progress.file_index + 1,
                                    progress.total_files,
                                    fmt_bytes(progress.current_file_bytes),
                                    fmt_bytes(progress.current_file_size),
                                    speed_str,
                                )
                            } else {
                                format!(
                                    "{} / {} files{}",
                                    progress.file_index + 1,
                                    progress.total_files,
                                    speed_str,
                                )
                            };
                            ui.label(
                                egui::RichText::new(stats_text)
                                    .font(mono(9.5))
                                    .color(TEXT_SECONDARY),
                            );

                            if let Some(err) = &progress.error {
                                ui.label(
                                    egui::RichText::new(format!("⚠  {err}"))
                                        .font(mono(9.5))
                                        .color(DANGER),
                                );
                            }

                            ui.add_space(2.0);

                            // Action button
                            if progress.is_complete || progress.error.is_some() {
                                if card_btn(ui, "OK", prop_medium(12.0), TEXT_PRIMARY).clicked() {
                                    event = Some(DownloadCardEvent::Dismiss);
                                }
                            } else if card_btn(ui, "Cancel", prop(12.0), TEXT_SECONDARY).clicked() {
                                event = Some(DownloadCardEvent::Cancel);
                            }
                        });
                    });
                });
        });

    event
}

fn card_btn(ui: &mut egui::Ui, label: &str, font: egui::FontId, color: Color32) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(label).font(font).color(color))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(1.0, TEXT_MUTED_55))
            .min_size(Vec2::new(80.0, 28.0))
            .corner_radius(egui::CornerRadius::same(0)),
    )
}

fn overall_frac(p: &DownloadProgress) -> f32 {
    if p.is_complete {
        return 1.0;
    }
    if p.bytes_total > 0 {
        return (p.bytes_done as f32 / p.bytes_total as f32).clamp(0.0, 1.0);
    }
    // Check phase: animate ring across the file manifest so the ring isn't static.
    if p.total_files > 0 {
        return ((p.file_index + 1) as f32 / p.total_files as f32).clamp(0.0, 1.0);
    }
    0.0
}

fn fmt_bytes(n: u64) -> String {
    if n >= 1_073_741_824 {
        format!("{:.1} GB", n as f64 / 1_073_741_824.0)
    } else if n >= 1_048_576 {
        format!("{:.0} MB", n as f64 / 1_048_576.0)
    } else {
        format!("{:.0} KB", n as f64 / 1024.0)
    }
}
