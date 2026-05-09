use egui::{Color32, Id, Rect, Sense, Stroke, Ui, pos2, vec2};

use super::style::*;

/// Horizontal menu row with number, label, meta text, and animated hover arrow.
/// `anim` is a persistent [0,1] float owned by the caller; mutated each frame.
/// Returns true if clicked.
pub fn menu_row(
    ui: &mut Ui,
    num: &str,
    label: &str,
    meta: &str,
    primary: bool,
    danger: bool,
    enabled: bool,
    anim: &mut f32,
) -> bool {
    let row_h = 44.0_f32;
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (response, painter) = ui.allocate_painter(vec2(ui.available_width(), row_h), sense);
    let rect = response.rect;

    let hovered = enabled && response.hovered();
    // target is 1.0 when hovered, 0.0 when not — the value we're animating toward
    let target = if hovered { 1.0_f32 } else { 0.0 };
    // lerp: close 22% of the gap each frame → fast ease-out that slows near the target
    *anim += (target - *anim) * 0.22;
    // keep requesting repaints until anim has settled; once it's within 0.002 of target
    // the visual difference is imperceptible and we stop driving the loop
    if (*anim - target).abs() > 0.002 {
        ui.ctx().request_repaint();
    }

    // shift all content right by up to 8px at full hover
    let pad = *anim * 8.0;

    // top hairline
    painter.line_segment(
        [rect.left_top(), rect.right_top()],
        Stroke::new(1.0, HAIRLINE),
    );

    let dim = !enabled;
    // number
    painter.text(
        pos2(rect.min.x + pad, rect.min.y + 14.0),
        egui::Align2::LEFT_TOP,
        num,
        mono(10.0),
        if dim { TEXT_MUTED_55 } else { TEXT_MUTED },
    );

    // label — color depends on hover/danger/primary flags
    let label_color = if dim {
        TEXT_MUTED_55
    } else if danger && hovered {
        DANGER
    } else if hovered {
        Color32::WHITE
    } else if primary {
        TEXT_PRIMARY
    } else {
        TEXT_SECONDARY
    };
    let label_font = if primary {
        prop_medium(17.0)
    } else {
        prop(17.0)
    };
    painter.text(
        pos2(rect.min.x + pad + 36.0, rect.min.y + 11.0),
        egui::Align2::LEFT_TOP,
        label,
        label_font,
        label_color,
    );

    // meta text (right-aligned)
    if !meta.is_empty() {
        painter.text(
            pos2(rect.max.x - 26.0, rect.min.y + 17.0),
            egui::Align2::RIGHT_TOP,
            meta,
            mono(10.0),
            TEXT_MUTED_55,
        );
    }

    if !dim {
        // arrow fades in (alpha 0→178) and slides right (x offset 0→6px) on hover
        let arrow_alpha = (*anim * 178.0) as u8;
        let arrow_x_offset = *anim * 6.0;
        if arrow_alpha > 2 {
            painter.text(
                pos2(rect.max.x - 8.0 + arrow_x_offset - 16.0, rect.min.y + 14.0),
                egui::Align2::RIGHT_TOP,
                "→",
                mono(12.0),
                Color32::from_rgba_premultiplied(178, 175, 170, arrow_alpha),
            );
        }
    }

    enabled && response.clicked()
}

/// Larger choice card (A / B style) with title, description, and size badge.
pub fn choice_item(
    ui: &mut Ui,
    num: &str,
    title: &str,
    desc: &str,
    size_badge: &str,
    checked: bool,
    anim: &mut f32,
) -> bool {
    let min_h = 88.0_f32;
    let avail_w = ui.available_width();
    let desc_galley = ui
        .ctx()
        .fonts_mut(|f| f.layout(desc.to_string(), prop(12.0), TEXT_MUTED_55, avail_w - 80.0));
    let total_h = (min_h + desc_galley.size().y).max(min_h);

    let (response, painter) = ui.allocate_painter(vec2(avail_w, total_h), Sense::click());
    let rect = response.rect;

    let hovered = response.hovered();
    let target = if hovered { 1.0_f32 } else { 0.0 };
    // same lerp + repaint-until-settled pattern as menu_row
    *anim += (target - *anim) * 0.22;
    if (*anim - target).abs() > 0.002 {
        ui.ctx().request_repaint();
    }

    let pad = *anim * 8.0;

    // Subtle green tint when this option is selected
    if checked {
        painter.rect_filled(
            rect,
            egui::CornerRadius::same(0),
            Color32::from_rgba_unmultiplied(GREEN_CHECKED.r(), GREEN_CHECKED.g(), GREEN_CHECKED.b(), 18),
        );
    }

    painter.line_segment(
        [rect.left_top(), rect.right_top()],
        Stroke::new(1.0, if checked { GREEN_CHECKED } else { HAIRLINE }),
    );

    painter.text(
        pos2(rect.min.x + pad, rect.min.y + 22.0),
        egui::Align2::LEFT_TOP,
        num,
        mono(10.0),
        if checked { GREEN_CHECKED } else { TEXT_MUTED },
    );

    let text_color = if hovered {
        Color32::WHITE
    } else if checked {
        TEXT_PRIMARY
    } else {
        TEXT_SECONDARY
    };
    painter.text(
        pos2(rect.min.x + pad + 38.0, rect.min.y + 18.0),
        egui::Align2::LEFT_TOP,
        title,
        prop(18.0),
        text_color,
    );

    // description (word-wrapped)
    painter.galley(
        pos2(rect.min.x + pad + 38.0, rect.min.y + 44.0),
        desc_galley,
        TEXT_MUTED_55,
    );

    // size badge
    painter.text(
        pos2(rect.min.x + pad + 38.0, rect.max.y - 16.0),
        egui::Align2::LEFT_BOTTOM,
        size_badge,
        mono(10.0),
        TEXT_MUTED_55,
    );

    // Checked indicator (replaces arrow) or animated arrow on hover
    if checked {
        let cx = rect.max.x - 18.0;
        let cy = rect.min.y + 20.0;
        painter.circle_filled(egui::pos2(cx, cy), 4.0, GREEN_CHECKED);
        painter.circle_stroke(
            egui::pos2(cx, cy),
            6.5,
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(GREEN_CHECKED.r(), GREEN_CHECKED.g(), GREEN_CHECKED.b(), 80)),
        );
    } else {
        // arrow
        let arrow_alpha = (*anim * 178.0) as u8;
        let arrow_x = *anim * 4.0;
        if arrow_alpha > 2 {
            painter.text(
                pos2(rect.max.x - 10.0 + arrow_x, rect.min.y + 22.0),
                egui::Align2::RIGHT_TOP,
                "→",
                mono(12.0),
                Color32::from_rgba_premultiplied(178, 175, 170, arrow_alpha),
            );
        }
    }

    response.clicked()
}

/// Segmented control (horizontal pill selector). `options`: display strings.
/// `current`: mutable index of active option. Returns true if changed.
pub fn segmented_control(ui: &mut Ui, id: Id, options: &[&str], current: &mut u32) -> bool {
    let item_w = {
        let max_label_w = options
            .iter()
            .map(|s| {
                ui.ctx().fonts_mut(|f| {
                    f.layout_no_wrap(s.to_string(), mono(10.0), Color32::WHITE)
                        .size()
                        .x
                })
            })
            .fold(0.0_f32, f32::max);
        (max_label_w + 20.0).max(44.0)
    };
    let h = 28.0_f32;
    let total_w = item_w * options.len() as f32;

    let (_, painter) = ui.allocate_painter(vec2(total_w, h), Sense::hover());
    let outer = painter.clip_rect();

    // outer border
    painter.rect_stroke(
        outer,
        egui::CornerRadius::same(2),
        Stroke::new(1.0, SEG_BORDER),
        egui::StrokeKind::Outside,
    );

    let mut changed = false;
    for (i, label) in options.iter().enumerate() {
        let item_rect = Rect::from_min_size(
            pos2(outer.min.x + i as f32 * item_w, outer.min.y),
            vec2(item_w, h),
        );
        let resp = ui.interact(item_rect, id.with(i), Sense::click());

        if resp.clicked() {
            *current = i as u32;
            changed = true;
        }

        let is_active = *current == i as u32;
        let is_hovered = resp.hovered();

        if is_active {
            painter.rect_filled(item_rect, egui::CornerRadius::same(0), SEG_ACTIVE);
        }

        if i + 1 < options.len() {
            painter.line_segment(
                [item_rect.right_top(), item_rect.right_bottom()],
                Stroke::new(1.0, HAIRLINE),
            );
        }

        let text_color = if is_active {
            Color32::WHITE
        } else if is_hovered {
            TEXT_PRIMARY
        } else {
            TEXT_MUTED_55
        };
        painter.text(
            item_rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            mono(10.0),
            text_color,
        );
    }
    changed
}

/// Checkbox styled as a 16×16 bordered square with a checkmark when active.
/// Returns true if the toggle was clicked this frame.
pub fn styled_checkbox(ui: &mut Ui, checked: &mut bool) -> bool {
    let size = 16.0_f32;
    let (response, painter) = ui.allocate_painter(vec2(size, size), Sense::click());
    let rect = response.rect;

    let border_color = if *checked {
        TEXT_PRIMARY
    } else {
        TEXT_MUTED_55
    };
    let fill = if *checked {
        SEG_ACTIVE
    } else {
        Color32::TRANSPARENT
    };

    painter.rect_filled(rect, egui::CornerRadius::same(0), fill);
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(0),
        Stroke::new(1.0, border_color),
        egui::StrokeKind::Inside,
    );

    if *checked {
        // checkmark path
        let p0 = rect.min + vec2(2.5, 8.0);
        let p1 = rect.min + vec2(6.0, 11.5);
        let p2 = rect.min + vec2(13.0, 3.5);
        let stroke = Stroke::new(1.6, TEXT_PRIMARY);
        painter.line_segment([p0, p1], stroke);
        painter.line_segment([p1, p2], stroke);
    }

    if response.clicked() {
        *checked = !*checked;
        true
    } else {
        false
    }
}

/// Small ⓘ circle with a tooltip that appears above on hover.
pub fn info_tooltip_button(ui: &mut Ui, id: Id, tooltip: &str) {
    let size = 13.0_f32;
    let (response, painter) = ui.allocate_painter(vec2(size, size), Sense::hover());
    let rect = response.rect;

    let hovered = response.hovered();
    let border_col = if hovered {
        Color32::WHITE
    } else {
        TEXT_MUTED_55
    };
    let text_col = if hovered {
        Color32::WHITE
    } else {
        TEXT_MUTED_55
    };

    painter.circle_stroke(rect.center(), size * 0.5, Stroke::new(1.0, border_col));
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "?",
        mono(9.0),
        text_col,
    );

    if hovered {
        let tip_w = 240.0_f32;
        let tip_padding = vec2(12.0, 10.0);
        let galley = ui.ctx().fonts_mut(|f| {
            f.layout(
                tooltip.to_string(),
                prop(12.0),
                Color32::from_rgba_premultiplied(229, 225, 217, 230),
                tip_w - tip_padding.x * 2.0,
            )
        });
        let tip_h = galley.size().y + tip_padding.y * 2.0;
        let screen = ui.ctx().content_rect();
        // Position above button, clamped to screen bounds
        let tip_x =
            (rect.center().x - tip_w * 0.5).clamp(screen.min.x + 4.0, screen.max.x - tip_w - 4.0);
        let tip_y = (rect.min.y - tip_h - 8.0).max(screen.min.y + 4.0);
        let tip_rect = Rect::from_min_size(pos2(tip_x, tip_y), vec2(tip_w, tip_h));

        // Layer painter at Tooltip order so the tooltip is not clipped by the widget's 13×13 clip rect
        let tip_painter = ui
            .ctx()
            .layer_painter(egui::LayerId::new(egui::Order::Tooltip, id.with("_tip")));
        tip_painter.rect_filled(
            tip_rect,
            egui::CornerRadius::same(4),
            super::style::TOOLTIP_BG,
        );
        tip_painter.rect_stroke(
            tip_rect,
            egui::CornerRadius::same(4),
            Stroke::new(1.0, HAIRLINE),
            egui::StrokeKind::Outside,
        );
        tip_painter.galley(tip_rect.min + tip_padding, galley, Color32::WHITE);

        let cx = tip_rect.center().x;
        let cy = tip_rect.max.y;
        let caret = egui::Shape::convex_polygon(
            vec![pos2(cx - 5.0, cy), pos2(cx + 5.0, cy), pos2(cx, cy + 5.0)],
            super::style::TOOLTIP_BG,
            Stroke::NONE,
        );
        tip_painter.add(caret);
    }
}

/// Styled dropdown selector. `options`: display strings. `current`: active index.
/// Returns true if selection changed.
pub fn dropdown(ui: &mut Ui, id: Id, options: &[&str], current: &mut u32) -> bool {
    let is_open = ui.data(|d| d.get_temp::<bool>(id).unwrap_or(false));
    let selected = options.get(*current as usize).copied().unwrap_or("");

    let max_label_w = options
        .iter()
        .map(|s| {
            ui.ctx().fonts_mut(|f| {
                f.layout_no_wrap(s.to_string(), mono(10.0), Color32::WHITE)
                    .size()
                    .x
            })
        })
        .fold(0.0_f32, f32::max);
    let btn_w = (max_label_w + 32.0).max(80.0);
    let btn_h = 28.0_f32;
    let item_h = 24.0_f32;

    let (response, painter) = ui.allocate_painter(vec2(btn_w, btn_h), Sense::click());
    let rect = response.rect;

    if is_open {
        painter.rect_filled(rect, egui::CornerRadius::same(2), SEG_ACTIVE);
    }
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(2),
        Stroke::new(1.0, SEG_BORDER),
        egui::StrokeKind::Outside,
    );
    painter.text(
        pos2(rect.min.x + 10.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        selected,
        mono(10.0),
        Color32::WHITE,
    );
    painter.text(
        pos2(rect.max.x - 8.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        if is_open { "▴" } else { "▾" },
        mono(9.0),
        TEXT_MUTED_55,
    );

    if response.clicked() {
        ui.data_mut(|d| d.insert_temp(id, !is_open));
    }

    let mut changed = false;

    if is_open {
        let popup_inner_w = btn_w - 4.0;
        let popup_result = egui::Area::new(id.with("_dd_popup"))
            .order(egui::Order::Tooltip)
            .fixed_pos(pos2(rect.min.x, rect.max.y + 2.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgba_premultiplied(20, 20, 22, 252))
                    .stroke(Stroke::new(1.0, SEG_BORDER))
                    .inner_margin(egui::Margin::same(2))
                    .show(ui, |ui| {
                        ui.set_min_width(popup_inner_w);
                        for (i, label) in options.iter().enumerate() {
                            let is_sel = *current == i as u32;
                            let (item_resp, item_painter) =
                                ui.allocate_painter(vec2(popup_inner_w, item_h), Sense::click());
                            let ir = item_resp.rect;
                            if is_sel || item_resp.hovered() {
                                item_painter.rect_filled(
                                    ir,
                                    egui::CornerRadius::same(0),
                                    SEG_ACTIVE,
                                );
                            }
                            item_painter.text(
                                pos2(ir.min.x + 8.0, ir.center().y),
                                egui::Align2::LEFT_CENTER,
                                *label,
                                mono(10.0),
                                if is_sel {
                                    Color32::WHITE
                                } else {
                                    TEXT_SECONDARY
                                },
                            );
                            if item_resp.clicked() {
                                *current = i as u32;
                                changed = true;
                                ui.data_mut(|d| d.insert_temp(id, false));
                            }
                        }
                    });
            });

        let popup_rect = popup_result.response.rect;
        let ptr = ui
            .ctx()
            .input(|i| i.pointer.latest_pos())
            .unwrap_or_default();
        if ui.ctx().input(|i| i.pointer.any_click())
            && !rect.contains(ptr)
            && !popup_rect.contains(ptr)
        {
            ui.data_mut(|d| d.insert_temp(id, false));
        }
    }

    changed
}

/// Thin horizontal divider rule used inside the brand section.
pub fn hairline_rule(ui: &mut Ui) {
    let (_, painter) = ui.allocate_painter(vec2(ui.available_width(), 1.0), Sense::hover());
    let rect = painter.clip_rect();
    painter.line_segment(
        [rect.left_top(), rect.right_top()],
        Stroke::new(1.0, HAIRLINE),
    );
}

/// Breadcrumb bar: "← Back / Current Screen"
/// Returns true if back was clicked.
pub fn breadcrumb(ui: &mut Ui, back_label: &str, current: &str) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        let back_resp = ui.add(
            egui::Label::new(
                egui::RichText::new(back_label)
                    .font(mono(10.0))
                    .color(TEXT_SECONDARY),
            )
            .sense(Sense::click()),
        );
        if back_resp.hovered() {
            ui.ctx()
                .output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
        }
        if back_resp.clicked() {
            clicked = true;
        }

        ui.label(egui::RichText::new("/").font(mono(10.0)).color(TEXT_MUTED));
        ui.label(
            egui::RichText::new(current)
                .font(mono(10.0))
                .color(TEXT_PRIMARY),
        );
    });
    clicked
}

/// Brand block: eyebrow, h1, subtitle.
pub fn brand_block(ui: &mut Ui, eyebrow: &str, title_plain: &str, title_dot: &str, subtitle: &str) {
    ui.add_space(2.0);
    ui.label(
        egui::RichText::new(eyebrow)
            .font(mono(10.0))
            .color(TEXT_MUTED),
    );
    ui.add_space(4.0);
    // h1 with regular + medium "."
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(title_plain)
                .font(prop(36.0))
                .color(TEXT_PRIMARY),
        );
        ui.label(
            egui::RichText::new(title_dot)
                .font(prop_medium(36.0))
                .color(TEXT_PRIMARY),
        );
    });
    ui.add_space(2.0);
    ui.label(
        egui::RichText::new(subtitle)
            .font(mono(10.0))
            .color(TEXT_MUTED),
    );
}

/// Footer row: left = status indicator, right = position info.
pub fn status_footer(ui: &mut Ui, status: &str, right: &str) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        // green dot
        let (dot_rect, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
        ui.painter()
            .circle_filled(dot_rect.center(), 3.0, super::style::GREEN_DOT);
        ui.painter().circle_stroke(
            dot_rect.center(),
            4.5,
            Stroke::new(1.0, Color32::from_rgba_premultiplied(108, 171, 122, 80)),
        );
        ui.label(
            egui::RichText::new(status)
                .font(mono(10.0))
                .color(TEXT_MUTED),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(right)
                    .font(mono(10.0))
                    .color(TEXT_MUTED),
            );
        });
    });
}
