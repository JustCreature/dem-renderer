//! CPU pipeline over in-memory `Heightmap`s, extracted from the binary's
//! `viewer/scene_init.rs`. The `&Path`/`File` reads that fed the original
//! `prepare_*` functions are platform-bound and live behind
//! [`crate::platform::TileSource`]; what remains here operates purely on
//! already-loaded heightmaps and is shared by the streaming tier workers.

use dem_io::{Heightmap, crop};

use crate::tiers::AO_RADIUS_M;

// Day 172 = June 21 (summer solstice). Must match sim_day / sim_hour in the
// ViewerCore init and the initial shadow computed at scene build — changing one
// without the others produces a mismatch between the displayed sun and the
// shadow map at startup.
pub const INIT_SIM_DAY: i32 = 172;
pub const INIT_SIM_HOUR: f32 = 10.0; // 10:00 AM solar time

/// Compute ambient occlusion for a 2×AO_RADIUS_M window centred on the camera,
/// then splat the result back into a full-heightmap-sized buffer (1.0 fill outside
/// the crop). This is ~27× faster than running AO over the entire heightmap.
pub fn compute_ao_cropped(hm: &Heightmap, cam_x: f64, cam_y: f64) -> Vec<f32> {
    let cam_col = (cam_x / hm.dx_meters) as isize;
    let cam_row = (cam_y / hm.dy_meters) as isize;
    let radius_px = (AO_RADIUS_M / hm.dx_meters) as isize;
    let row_start = (cam_row - radius_px).max(0) as usize;
    let col_start = (cam_col - radius_px).max(0) as usize;
    let crop_rows =
        ((cam_row + radius_px).min(hm.rows as isize) - row_start as isize).max(0) as usize;
    let crop_cols =
        ((cam_col + radius_px).min(hm.cols as isize) - col_start as isize).max(0) as usize;
    let cropped_hm = crop(hm, row_start, col_start, crop_rows, crop_cols);
    let crop_ao = terrain::compute_ao_true_hemi(&cropped_hm, 16, 10.0f32.to_radians(), 200.0);
    let mut ao = vec![1.0f32; hm.rows * hm.cols];
    for r in 0..crop_rows {
        let dst = (row_start + r) * hm.cols + col_start;
        ao[dst..dst + crop_cols].copy_from_slice(&crop_ao[r * crop_cols..(r + 1) * crop_cols]);
    }
    ao
}
