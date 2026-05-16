use std::path::Path;
use std::sync::Arc;

use dem_io::{Heightmap, crop, extract_window, load_grid};
use render_gpu::{GpuContext, GpuScene};

use super::geo::{latlon_to_tile_metres, sun_position};
use super::tiers::{AO_RADIUS_M, BEV_BASE_RADIUS_M, cap_to_gpu_limit, select_ifd};
use crate::consts::GPU_SAFE_PX;

// Day 172 = June 21 (summer solstice). Must match sim_day / sim_hour in the Viewer init
// and the initial shadow computed by prepare_scene — changing one without the others
// produces a mismatch between the displayed sun and the shadow map at startup.
pub(super) const INIT_SIM_DAY: i32 = 172;
pub(super) const INIT_SIM_HOUR: f32 = 10.0; // 10:00 AM solar time

/// Compute ambient occlusion for a 2×AO_RADIUS_M window centred on the camera,
/// then splat the result back into a full-heightmap-sized buffer (1.0 fill outside
/// the crop). This is ~27× faster than running AO over the entire heightmap.
pub(super) fn compute_ao_cropped(hm: &Heightmap, cam_x: f64, cam_y: f64) -> Vec<f32> {
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

/// Like `prepare_scene` but reuses an existing `GpuContext` (for seamless surface handoff)
/// and accepts a progress callback `report(fraction, label)` called after each major step.
pub(crate) fn prepare_scene_with_ctx(
    gpu_ctx: GpuContext,
    tile_path: &Path,
    width: u32,
    height: u32,
    cam_lat: f64,
    cam_lon: f64,
    report: impl Fn(f32, &str),
) -> crate::viewer::PreparedScene {
    report(0.05, "Reading terrain data…");

    // Detect GLO-30 by directory naming convention, not pixel scale.
    let is_glo30 = tile_path
        .to_str()
        .map_or(false, |s| s.contains("Copernicus_DSM_COG_"));

    let hm = if is_glo30 {
        let tiles_dir = tile_path
            .parent()
            .and_then(|p| p.parent())
            .unwrap_or(Path::new("tiles"));
        let centre_lat = cam_lat.floor() as i32;
        let centre_lon = cam_lon.floor() as i32;
        let t0 = std::time::Instant::now();
        let hm = load_grid(tiles_dir, centre_lat, centre_lon, |p| {
            dem_io::parse_geotiff_auto(p).ok()
        });
        println!(
            "GLO-30 3×3 grid: {}×{} at {:.4}°/px  ({:.2?})",
            hm.cols,
            hm.rows,
            hm.dx_deg,
            t0.elapsed()
        );
        hm
    } else {
        let proj4 = dem_io::crs::tile_proj4(tile_path).expect("failed to resolve CRS from tile");
        let centre_crs = dem_io::crs::from_wgs84(cam_lat, cam_lon, &proj4)
            .or_else(|_| dem_io::tile_centre_crs(tile_path))
            .unwrap_or_else(|_| (0.0, 0.0)); // unreachable in practice; or_else third arm handles it
        let t0 = std::time::Instant::now();
        let scales = dem_io::ifd_scales(tile_path).unwrap_or_else(|_| vec![1.0]);
        let base_ifd = select_ifd(&scales, 30.0, BEV_BASE_RADIUS_M, GPU_SAFE_PX as u32);
        let loaded = match extract_window(tile_path, centre_crs, BEV_BASE_RADIUS_M, base_ifd)
            .or_else(|_| extract_window(tile_path, centre_crs, BEV_BASE_RADIUS_M, 1))
            .or_else(|_| {
                // Camera outside tile — retry from tile geographic centre
                dem_io::tile_centre_crs(tile_path)
                    .and_then(|tc| extract_window(tile_path, tc, BEV_BASE_RADIUS_M, base_ifd))
            }) {
            Ok(hm) => {
                println!(
                    "window: {}×{} at {:.1}m/px, elev {:.0}–{:.0}m  ({:.2?})",
                    hm.cols,
                    hm.rows,
                    hm.dx_meters,
                    hm.data.iter().cloned().fold(f32::INFINITY, f32::min),
                    hm.data.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
                    t0.elapsed(),
                );
                hm
            }
            Err(_) => {
                let hm = dem_io::parse_geotiff_auto(tile_path)
                    .expect("parse_geotiff_auto failed — check tile path");
                println!(
                    "full tile: {}×{} at {:.1}m/px, elev {:.0}–{:.0}m  ({:.2?})",
                    hm.cols,
                    hm.rows,
                    hm.dx_meters,
                    hm.data.iter().cloned().fold(f32::INFINITY, f32::min),
                    hm.data.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
                    t0.elapsed(),
                );
                hm
            }
        };
        // Crop to GPU-safe size when tile or clipped window still exceeds the limit.
        // This happens for high-res tiles with no overviews (e.g. 1m NZ LiDAR, 24000 px wide).
        if loaded.cols > GPU_SAFE_PX || loaded.rows > GPU_SAFE_PX {
            println!(
                "cropping oversized tile {}×{} → {}×{}",
                loaded.cols,
                loaded.rows,
                GPU_SAFE_PX.min(loaded.cols),
                GPU_SAFE_PX.min(loaded.rows)
            );
        }
        let (centre_e, centre_n) = latlon_to_tile_metres(cam_lat, cam_lon, &loaded)
            .map(|(x, y)| {
                (
                    loaded.crs_origin_x + x as f64,
                    loaded.crs_origin_y - y as f64,
                )
            })
            .unwrap_or((
                loaded.crs_origin_x + loaded.cols as f64 * loaded.dx_meters * 0.5,
                loaded.crs_origin_y - loaded.rows as f64 * loaded.dy_meters * 0.5,
            ));
        cap_to_gpu_limit(loaded, centre_e, centre_n)
    };

    report(0.30, "Computing surface normals…");
    let t1 = std::time::Instant::now();
    let normal_map = terrain::compute_normals_vector_par(&hm);
    println!("normals:  {:.2?}", t1.elapsed());

    let lat_rad = (cam_lat as f32).to_radians();
    let (init_az, init_el) = sun_position(lat_rad, INIT_SIM_DAY, INIT_SIM_HOUR);

    report(0.50, "Computing sun shadows…");
    let t2 = std::time::Instant::now();
    let shadow_mask = terrain::compute_shadow_vector_par_with_azimuth(&hm, init_az, init_el, 200.0);
    println!("shadows:  {:.2?}", t2.elapsed());

    let (cam_x, cam_y) = latlon_to_tile_metres(cam_lat, cam_lon, &hm)
        .map(|(x, y)| (x as f64, y as f64))
        .unwrap_or((
            hm.cols as f64 * hm.dx_meters * 0.5,
            hm.rows as f64 * hm.dy_meters * 0.5,
        ));

    report(0.70, "Computing ambient occlusion…");
    let t3 = std::time::Instant::now();
    let ao_data_mask = compute_ao_cropped(&hm, cam_x, cam_y);
    println!("ao:       {:.2?}", t3.elapsed());

    report(0.90, "Uploading to GPU…");
    let hm = Arc::new(hm);
    let scene: GpuScene = GpuScene::new(
        gpu_ctx,
        &hm,
        &normal_map,
        &shadow_mask,
        &ao_data_mask,
        width,
        height,
    );

    crate::viewer::PreparedScene {
        scene,
        hm,
        lat_rad,
        width,
        height,
    }
}
