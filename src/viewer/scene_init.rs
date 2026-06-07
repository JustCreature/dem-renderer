use std::path::Path;
use std::sync::Arc;

use dem_io::{Heightmap, crop, extract_window, load_grid_from_paths};
use render_gpu::{GpuContext, GpuScene};

use super::geo::{latlon_to_tile_metres, sun_position};
use super::tiers::{AO_RADIUS_M, cap_to_gpu_limit, select_ifd, tier_radii};
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

/// Initial scene for demo mode: loads an N×M Copernicus grid from `demo_view.base_tile_paths`,
/// then also builds the overview cache for each close tile so the close worker starts fast.
pub(crate) fn prepare_demo_scene_with_ctx(
    gpu_ctx: GpuContext,
    demo_view: &crate::launcher::config::DemoViewConfig,
    width: u32,
    height: u32,
    report: impl Fn(f32, &str),
) -> crate::viewer::PreparedScene {
    let cam_lat = demo_view.camera_lat;
    let cam_lon = demo_view.camera_lon;

    report(0.10, "Reading terrain data…");
    let t0 = std::time::Instant::now();
    let mut hm = load_grid_from_paths(&demo_view.base_tile_paths, |p| {
        dem_io::parse_geotiff_auto(p).ok()
    });
    dem_io::clamp_nodata_to_sea(&mut hm);
    let n_tiles = demo_view.base_tile_paths.len();
    let (raw_cols, raw_rows) = (hm.cols, hm.rows);
    // Cap the assembled grid to GPU_SAFE_PX centered on the camera before any
    // downstream work — normals/shadows/AO/GPU upload all scale with grid area,
    // and texture dimensions above the adapter limit (8 192–16 384 depending on
    // hardware) fail validation. cap_to_gpu_limit no-ops when the grid already
    // fits, so 3×3 demos behave exactly as before.
    hm = cap_to_gpu_limit(hm, cam_lon, cam_lat);
    println!(
        "demo base grid: {}×{} ({} tiles, cropped from {}×{}) at {:.4}°/px  ({:.2?})",
        hm.cols,
        hm.rows,
        n_tiles,
        raw_cols,
        raw_rows,
        hm.dx_deg,
        t0.elapsed()
    );

    report(0.65, "Computing surface normals…");
    let t1 = std::time::Instant::now();
    let normal_map = terrain::compute_normals_vector_par(&hm);
    println!("normals:  {:.2?}", t1.elapsed());

    let lat_rad = (cam_lat as f32).to_radians();
    let (init_az, init_el) = sun_position(lat_rad, INIT_SIM_DAY, INIT_SIM_HOUR);

    report(0.75, "Computing sun shadows…");
    let t2 = std::time::Instant::now();
    let shadow_mask = terrain::compute_shadow_vector_par_with_azimuth(&hm, init_az, init_el, 200.0);
    println!("shadows:  {:.2?}", t2.elapsed());

    let cam_x = (cam_lon - hm.crs_origin_x) / hm.dx_deg * hm.dx_meters;
    let cam_y = (hm.crs_origin_y - cam_lat) / hm.dy_deg.abs() * hm.dy_meters;

    report(0.85, "Computing ambient occlusion…");
    let t3 = std::time::Instant::now();
    let ao_data_mask = compute_ao_cropped(&hm, cam_x, cam_y);
    println!("ao:       {:.2?}", t3.elapsed());

    report(0.95, "Uploading to GPU…");
    let hm = Arc::new(hm);
    let scene = GpuScene::new(
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
        cache_path: None,
    }
}

/// Like `prepare_scene` but reuses an existing `GpuContext` (for seamless surface handoff)
/// and accepts a progress callback `report(fraction, label)` called after each major step.
///
/// `vram_budget` drives the initial base-tier extract radius so a Low preset doesn't read
/// a 90 km window only to crop it. The adapter-derived `gpu_ctx.vram_class` is logged for
/// context but the user's choice always wins.
// Args are the distinct scene-preparation inputs (paths, camera, settings, gpu ctx).
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_scene_with_ctx(
    gpu_ctx: GpuContext,
    tile_path: &Path,
    width: u32,
    height: u32,
    cam_lat: f64,
    cam_lon: f64,
    vram_budget: crate::launcher::config::VramBudget,
    report: impl Fn(f32, &str),
) -> crate::viewer::PreparedScene {
    let (mut hm, cache_path) = {
        let proj4 = dem_io::crs::tile_proj4(tile_path).expect("failed to resolve CRS from tile");
        let is_geo = dem_io::crs::is_geographic(&proj4);

        if is_geo {
            // Geographic tile (e.g. Copernicus GLO-30, SRTM): extract_window stores the TIFF
            // pixel-scale tag verbatim as dx_meters, which for WGS84 tiles is degrees/px —
            // the shader would see a sub-metre terrain. Use parse_geotiff_auto instead, which
            // correctly converts degrees → metres via cos(lat) × M_PER_DEG.
            report(0.50, "Reading terrain data…");
            let t0 = std::time::Instant::now();
            let hm = dem_io::parse_geotiff_auto(tile_path)
                .expect("parse_geotiff_auto failed — check tile path");
            println!(
                "geographic tile: {}×{} at {:.4}°/px ({:.1}m/px)  ({:.2?})",
                hm.cols,
                hm.rows,
                hm.dx_deg,
                hm.dx_meters,
                t0.elapsed()
            );
            let (centre_lon, centre_lat) =
                dem_io::crs::from_wgs84(cam_lat, cam_lon, &proj4).unwrap_or((cam_lon, cam_lat));
            (cap_to_gpu_limit(hm, centre_lon, centre_lat), None)
        } else {
            let centre_crs = dem_io::crs::from_wgs84(cam_lat, cam_lon, &proj4)
                .or_else(|_| dem_io::tile_centre_crs(tile_path))
                .unwrap_or((0.0, 0.0));

            // For projected high-res single-IFD tiles: build the overview cache NOW, before
            // loading any pixel data.  For a 10 GB source this avoids reading the full tile
            // only to crop it; the small cache makes the initial load fast and consistent
            // with worker reloads.  Progress maps 0.05–0.48 so the bar stays monotonic.
            // Silently swallowing the cache build error (`.unwrap_or(None)`) made
            // issue #40 impossible to triage from a user report — the only sign
            // anything had gone wrong was a panic four fallbacks deep.  Surface
            // the error on both stderr and the loading screen.
            let cache_path: Option<std::path::PathBuf> =
                match dem_io::ensure_overview_cache(tile_path, |f, msg| {
                    report(0.05 + f * 0.43, msg);
                }) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("[cache] build failed for {}: {e}", tile_path.display());
                        report(
                            0.48,
                            &format!("Cache build failed: {e}; falling back to slow path"),
                        );
                        None
                    }
                };

            // Use the cache when available; otherwise fall back to the original tile.
            let tier_path: &Path = cache_path.as_deref().unwrap_or(tile_path);
            let t0 = std::time::Instant::now();
            report(0.50, "Reading terrain data…");
            let scales = dem_io::ifd_scales(tier_path).unwrap_or_else(|_| vec![1.0]);
            // Initial base load uses the chosen VRAM-class radius so a Low
            // preset doesn't waste time reading a 90 km window we'd immediately
            // crop. The adapter-detected class (gpu_ctx.vram_class) is purely
            // informational — the user's choice in vram_budget always wins.
            let init_radii = tier_radii(vram_budget.to_class());
            let base_radius = init_radii.base_radius_m;
            let base_ifd = select_ifd(&scales, 30.0, base_radius, GPU_SAFE_PX as u32);
            // The original code discarded each `Err` via `or_else(|_| ...)` and
            // then called `.expect("parse_geotiff_auto failed — check tile path")`
            // four levels deep.  Issue #40 surfaced as that generic panic with
            // no clue about the actual LZW decode failure.  Keep the first real
            // error so it can be reported alongside the final fallback's error.
            let mut first_err: Option<String> = None;
            let attempt = extract_window(tier_path, centre_crs, base_radius, base_ifd)
                .or_else(|e| {
                    first_err = Some(format!("extract_window(ifd={base_ifd}): {e}"));
                    extract_window(tier_path, centre_crs, base_radius, 1)
                })
                .or_else(|e| {
                    if first_err.is_none() {
                        first_err = Some(format!("extract_window(ifd=1): {e}"));
                    }
                    // Camera outside tile — retry from tile geographic centre
                    dem_io::tile_centre_crs(tier_path)
                        .and_then(|tc| extract_window(tier_path, tc, base_radius, base_ifd))
                });
            let loaded = match attempt {
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
                Err(last_err) => match dem_io::parse_geotiff_auto(tile_path) {
                    Ok(hm) => {
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
                    Err(parse_err) => {
                        let first =
                            first_err.unwrap_or_else(|| format!("extract_window: {last_err}"));
                        let full = format!(
                            "Could not load tile {}.\n  First failure: {first}\n  Final fallback (parse_geotiff_auto): {parse_err}",
                            tile_path.display()
                        );
                        eprintln!("{full}");
                        report(1.0, &full);
                        panic!("{full}");
                    }
                },
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
            (cap_to_gpu_limit(loaded, centre_e, centre_n), cache_path)
        }
    };
    dem_io::clamp_nodata_to_sea(&mut hm);

    let lat_rad = (cam_lat as f32).to_radians();
    let (cam_x, cam_y) = latlon_to_tile_metres(cam_lat, cam_lon, &hm)
        .map(|(x, y)| (x as f64, y as f64))
        .unwrap_or((
            hm.cols as f64 * hm.dx_meters * 0.5,
            hm.rows as f64 * hm.dy_meters * 0.5,
        ));

    report(0.65, "Computing surface normals…");
    let t1 = std::time::Instant::now();
    let normal_map = terrain::compute_normals_vector_par(&hm);
    println!("normals:  {:.2?}", t1.elapsed());

    let (init_az, init_el) = sun_position(lat_rad, INIT_SIM_DAY, INIT_SIM_HOUR);

    report(0.75, "Computing sun shadows…");
    let t2 = std::time::Instant::now();
    let shadow_mask = terrain::compute_shadow_vector_par_with_azimuth(&hm, init_az, init_el, 200.0);
    println!("shadows:  {:.2?}", t2.elapsed());

    report(0.85, "Computing ambient occlusion…");
    let t3 = std::time::Instant::now();
    let ao_data_mask = compute_ao_cropped(&hm, cam_x, cam_y);
    println!("ao:       {:.2?}", t3.elapsed());

    report(0.95, "Uploading to GPU…");
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
        cache_path,
    }
}
