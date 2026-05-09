use egui::{Color32, Mesh, Painter, Rect, TextureId, epaint::Vertex, pos2, vec2};

use super::style::{TEXT_MUTED, mono};

/// Full-window background photo, cover-filled and left-anchored.
pub fn draw_background(painter: &Painter, texture_id: TextureId, img_w: f32, img_h: f32) {
    let r = painter.clip_rect();
    let win_aspect = r.width() / r.height();
    let img_aspect = img_w / img_h;

    let uv = if img_aspect > win_aspect {
        let visible = win_aspect / img_aspect;
        Rect::from_min_max(pos2(0.0, 0.0), pos2(visible, 1.0))
    } else {
        let visible = win_aspect / img_aspect;
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, (1.0 / visible).min(1.0)))
    };

    painter.image(texture_id, r, uv, Color32::WHITE);
}

/// Horizontal gradient overlay matching the CSS photo-shade.
pub fn draw_gradient_shade(painter: &Painter) {
    let r = painter.clip_rect();
    let stops: &[(f32, Color32)] = &[
        (0.0, Color32::TRANSPARENT),
        (0.38, Color32::TRANSPARENT),
        (0.58, Color32::from_rgba_premultiplied(2, 2, 3, 140)),
        (1.0, Color32::from_rgba_premultiplied(5, 5, 6, 209)),
    ];

    let mut mesh = Mesh::with_texture(TextureId::default());
    for pair in stops.windows(2) {
        let (x0f, c0) = pair[0];
        let (x1f, c1) = pair[1];
        let x0 = r.min.x + x0f * r.width();
        let x1 = r.min.x + x1f * r.width();
        let base = mesh.vertices.len() as u32;
        let uv = pos2(0.0, 0.0);
        mesh.vertices.push(Vertex {
            pos: pos2(x0, r.min.y),
            uv,
            color: c0,
        });
        mesh.vertices.push(Vertex {
            pos: pos2(x1, r.min.y),
            uv,
            color: c1,
        });
        mesh.vertices.push(Vertex {
            pos: pos2(x1, r.max.y),
            uv,
            color: c1,
        });
        mesh.vertices.push(Vertex {
            pos: pos2(x0, r.max.y),
            uv,
            color: c0,
        });
        mesh.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    painter.add(egui::Shape::Mesh(std::sync::Arc::new(mesh)));
}

/// Radial vignette approximated with a vertex-coloured grid mesh.
pub fn draw_vignette(painter: &Painter) {
    let r = painter.clip_rect();
    let focus = vec2(0.25, 0.35);

    const GRID: usize = 6;
    let mut mesh = Mesh::with_texture(TextureId::default());
    let uv = pos2(0.0, 0.0);

    for iy in 0..GRID {
        for ix in 0..GRID {
            let fx = ix as f32 / (GRID - 1) as f32;
            let fy = iy as f32 / (GRID - 1) as f32;
            let dx = fx - focus.x;
            let dy = fy - focus.y;
            let dist = (dx * dx + dy * dy).sqrt() / 0.85;
            let t = ((dist - 0.3) / 0.7).clamp(0.0, 1.0);
            let alpha = (t * t * 140.0) as u8;
            let color = Color32::from_rgba_premultiplied(0, 0, 0, alpha);
            let pos = pos2(r.min.x + fx * r.width(), r.min.y + fy * r.height());
            mesh.vertices.push(Vertex { pos, uv, color });
        }
    }
    for iy in 0..(GRID - 1) {
        for ix in 0..(GRID - 1) {
            let tl = (iy * GRID + ix) as u32;
            let tr = tl + 1;
            let bl = tl + GRID as u32;
            let br = bl + 1;
            mesh.indices.extend_from_slice(&[tl, tr, br, tl, br, bl]);
        }
    }
    painter.add(egui::Shape::Mesh(std::sync::Arc::new(mesh)));
}

/// Four L-shaped corner registration marks.
pub fn draw_corner_marks(painter: &Painter) {
    let r = painter.clip_rect();
    let m = 24.0_f32;
    let s = 14.0_f32;
    let stroke = egui::Stroke::new(1.0, Color32::from_rgba_premultiplied(90, 88, 84, 102));

    // top-left
    painter.line_segment(
        [
            pos2(r.min.x + m, r.min.y + m + s),
            pos2(r.min.x + m, r.min.y + m),
        ],
        stroke,
    );
    painter.line_segment(
        [
            pos2(r.min.x + m, r.min.y + m),
            pos2(r.min.x + m + s, r.min.y + m),
        ],
        stroke,
    );
    // top-right
    painter.line_segment(
        [
            pos2(r.max.x - m - s, r.min.y + m),
            pos2(r.max.x - m, r.min.y + m),
        ],
        stroke,
    );
    painter.line_segment(
        [
            pos2(r.max.x - m, r.min.y + m),
            pos2(r.max.x - m, r.min.y + m + s),
        ],
        stroke,
    );
    // bottom-left
    painter.line_segment(
        [
            pos2(r.min.x + m, r.max.y - m - s),
            pos2(r.min.x + m, r.max.y - m),
        ],
        stroke,
    );
    painter.line_segment(
        [
            pos2(r.min.x + m, r.max.y - m),
            pos2(r.min.x + m + s, r.max.y - m),
        ],
        stroke,
    );
    // bottom-right
    painter.line_segment(
        [
            pos2(r.max.x - m - s, r.max.y - m),
            pos2(r.max.x - m, r.max.y - m),
        ],
        stroke,
    );
    painter.line_segment(
        [
            pos2(r.max.x - m, r.max.y - m),
            pos2(r.max.x - m, r.max.y - m - s),
        ],
        stroke,
    );
}

/// Top-left and bottom-left cartographic metadata labels.
pub fn draw_metadata_labels(painter: &Painter, title: &str, lat: f64, lon: f64, elev: Option<f64>) {
    let r = painter.clip_rect();
    let color = TEXT_MUTED;

    painter.text(
        pos2(r.min.x + 48.0, r.min.y + 28.0),
        egui::Align2::LEFT_TOP,
        title,
        mono(10.0),
        color,
    );

    let mut parts = vec![
        format!("LAT {:.2}°N", lat.abs()),
        format!("LON {:.2}°E", lon.abs()),
    ];
    if let Some(e) = elev {
        parts.push(format!("ELEV {} M", e as i64));
    }

    let mut x = r.min.x + 48.0;
    let y = r.max.y - 28.0;
    for part in &parts {
        painter.text(
            pos2(x, y),
            egui::Align2::LEFT_BOTTOM,
            part,
            mono(10.0),
            color,
        );
        x += painter
            .layout_no_wrap(part.clone(), mono(10.0), color)
            .size()
            .x
            + 18.0;
    }
}
