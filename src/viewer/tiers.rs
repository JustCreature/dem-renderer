use std::path::Path;
use std::sync::{Arc, mpsc};

use dem_io::{Heightmap, crop, extract_window, load_grid, parse_geotiff_auto, stitch_windows};
use render_gpu::GpuScene;
use terrain::{NormalMap, ShadowMask};

use super::geo::sun_position;
use super::scene_init::{INIT_SIM_DAY, INIT_SIM_HOUR, compute_ao_cropped};
use crate::consts::GPU_SAFE_PX;

// BEV COG (DGM_R5.tif) tier geometry.
pub(super) const BEV_BASE_RADIUS_M: f64 = 90_000.0;
// Camera must stay inside BEV_BASE_RADIUS_M − BEV_BASE_DRIFT_THRESHOLD_M from the window edge
pub(super) const BEV_BASE_DRIFT_THRESHOLD_M: f64 = 30_000.0;
pub(super) const BEV_5M_RADIUS_M: f64 = 20_000.0;
pub(super) const BEV_5M_DRIFT_THRESHOLD_M: f64 = 3_000.0;
pub(super) const BEV_1M_RADIUS_M: f64 = 3_500.0;
pub(super) const BEV_1M_DRIFT_THRESHOLD_M: f64 = 1_000.0;
// BEV tiles are named CRS3035RES50000m… — each covers exactly 50 km × 50 km in EPSG:3035.
pub(super) const BEV_TILE_SIZE_M: f64 = 50_000.0;

/// Scan `dir` recursively for `CRS3035RES50000mN{N}E{E}.tif` files and return all tiles
/// whose 50 km bounds overlap the window centred at `(e_centre, n_centre)` with the given radius.
pub(super) fn find_1m_tiles(
    dir: &Path,
    e_centre: f64,
    n_centre: f64,
    radius_m: f64,
) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let Ok(walker) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in walker.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(find_1m_tiles(&path, e_centre, n_centre, radius_m));
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with("CRS3035RES50000m") || !name.ends_with(".tif") {
            continue;
        }
        let Some(rest) = name
            .strip_prefix("CRS3035RES50000m")
            .and_then(|r| r.strip_suffix(".tif"))
        else {
            continue;
        };
        let Some(n_pos) = rest.find('N') else {
            continue;
        };
        let Some(e_pos) = rest.find('E') else {
            continue;
        };
        if n_pos >= e_pos {
            continue;
        }
        let Ok(tile_n): Result<f64, _> = rest[n_pos + 1..e_pos].parse() else {
            continue;
        };
        let Ok(tile_e): Result<f64, _> = rest[e_pos + 1..].parse() else {
            continue;
        };
        // tile covers [tile_e, tile_e+BEV_TILE_SIZE_M) × [tile_n, tile_n+BEV_TILE_SIZE_M)
        if tile_e < e_centre + radius_m
            && tile_e + BEV_TILE_SIZE_M > e_centre - radius_m
            && tile_n < n_centre + radius_m
            && tile_n + BEV_TILE_SIZE_M > n_centre - radius_m
        {
            found.push(path);
        }
    }
    found
}

/// Walk `dir` recursively and return the path of the first tile that matches the
/// `CRS3035RES50000m*.tif` naming convention used by `find_1m_tiles`.
/// Searching for the same pattern avoids accidentally reading the CRS from an
/// unrelated tile (e.g. DGM_R5.tif in a different projection) which would produce
/// wrong coordinates for the subsequent `find_1m_tiles` call.
fn find_first_1m_tile(dir: &Path) -> Option<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return None;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_first_1m_tile(&path) {
                return Some(found);
            }
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .map_or(false, |n| {
                n.starts_with("CRS3035RES50000m") && n.ends_with(".tif")
            })
        {
            return Some(path);
        }
    }
    None
}

/// Crop a heightmap to at most `GPU_SAFE_PX × GPU_SAFE_PX` pixels centered on
/// `(centre_e, centre_n)`.  No-op when the heightmap already fits.  Applied before
/// every GPU upload so that tiles with no overviews (e.g. 1m NZ LiDAR, 24000px wide)
/// never exceed wgpu's texture dimension limit.
pub(super) fn cap_to_gpu_limit(hm: Heightmap, centre_e: f64, centre_n: f64) -> Heightmap {
    if hm.cols <= GPU_SAFE_PX && hm.rows <= GPU_SAFE_PX {
        return hm;
    }
    let cam_col =
        ((centre_e - hm.crs_origin_x) / hm.dx_meters).clamp(0.0, (hm.cols - 1) as f64) as usize;
    let cam_row =
        ((hm.crs_origin_y - centre_n) / hm.dy_meters).clamp(0.0, (hm.rows - 1) as f64) as usize;
    let out_cols = GPU_SAFE_PX.min(hm.cols);
    let out_rows = GPU_SAFE_PX.min(hm.rows);
    let col_start = cam_col.saturating_sub(out_cols / 2).min(hm.cols - out_cols);
    let row_start = cam_row.saturating_sub(out_rows / 2).min(hm.rows - out_rows);
    crop(&hm, row_start, col_start, out_rows, out_cols)
}

pub(super) const AO_RADIUS_M: f64 = 20_000.0;
// AO_RADIUS_M − AO_DRIFT_THRESHOLD_M = minimum margin of valid AO data behind the camera
pub(super) const AO_DRIFT_THRESHOLD_M: f64 = 5_000.0;

/// Common result sent by any BEV background streaming worker.
/// The worker always provides the window-centre CRS coordinates so the
/// event loop can update drift tracking without tier-specific logic.
pub(super) struct TierData {
    pub(super) hm: Arc<Heightmap>,
    pub(super) normals: NormalMap,
    pub(super) shadow: ShadowMask,
    pub(super) ao: Vec<f32>,  // empty Vec for tiers that do not compute AO
    pub(super) centre_e: f64, // absolute CRS easting of the loaded window centre
    pub(super) centre_n: f64, // absolute CRS northing of the loaded window centre
}

/// Per-tier channel state and drift-detection bookkeeping.
///
/// One instance replaces the `{base,5m}_{tx,rx,computing,last_cx,last_cy}` field
/// groups that would otherwise be duplicated for every resolution tier.
/// Adding a new tier = create one more `StreamingTier` with the right thresholds.
pub(super) struct StreamingTier {
    pub(super) tx: mpsc::SyncSender<(f64, f64)>,
    rx: mpsc::Receiver<TierData>,
    pub(super) computing: bool,
    last_cx: f64,
    last_cy: f64,
    drift_threshold_m: f64,
}

impl StreamingTier {
    pub(super) fn new(
        tx: mpsc::SyncSender<(f64, f64)>,
        rx: mpsc::Receiver<TierData>,
        init_cx: f64,
        init_cy: f64,
        drift_threshold_m: f64,
    ) -> Self {
        StreamingTier {
            tx,
            rx,
            computing: false,
            last_cx: init_cx,
            last_cy: init_cy,
            drift_threshold_m,
        }
    }

    /// True when the camera has drifted far enough from the last window centre
    /// that a reload is warranted.
    pub(super) fn needs_reload(&self, e: f64, n: f64) -> bool {
        (e - self.last_cx).abs() > self.drift_threshold_m
            || (n - self.last_cy).abs() > self.drift_threshold_m
    }

    /// Send a reload request to the background worker.
    /// Sets `computing = true` on success and returns true.
    pub(super) fn try_trigger(&mut self, e: f64, n: f64) -> bool {
        if self.tx.try_send((e, n)).is_ok() {
            self.computing = true;
            true
        } else {
            false
        }
    }

    /// Poll for a finished bundle. On success, clears `computing` and
    /// updates `last_cx`/`last_cy` from the bundle's centre coordinates.
    pub(super) fn try_recv(&mut self) -> Option<TierData> {
        match self.rx.try_recv() {
            Ok(data) => {
                self.computing = false;
                self.last_cx = data.centre_e;
                self.last_cy = data.centre_n;
                Some(data)
            }
            Err(_) => None,
        }
    }

    /// Force-reset drift tracking so `needs_reload` returns true on the next check.
    /// Call this when the base heightmap swaps: the close tier's tile-local offsets
    /// become stale and it must reload immediately regardless of camera position.
    /// Setting last_cx/cy to 0.0 guarantees the check fires (Austrian CRS values
    /// are at ~4.4 M easting, far from zero).
    pub(super) fn invalidate(&mut self) {
        self.computing = false;
        self.last_cx = 0.0;
        self.last_cy = 0.0;
    }
}

/// Result sent by the GLO-30 background tile-slide worker to the event loop when
/// a new 3×3 Copernicus tile grid finishes loading.
pub(super) struct TileBundle {
    pub(super) hm: Arc<Heightmap>,
    pub(super) normals: NormalMap,
    pub(super) shadow: ShadowMask,
    pub(super) ao: Vec<f32>,
    pub(super) centre_lat: i32,
    pub(super) centre_lon: i32,
    pub(super) cam_lat: f64,
    pub(super) cam_lon: f64,
}

/// Persistent state for GLO-30 sliding-tile mode.
/// Tracks which 1°×1° tile is currently loaded and owns the worker channel pair.
pub(super) struct Glo30State {
    pub(super) centre_lat: i32,
    pub(super) centre_lon: i32,
    pub(super) tile_tx: mpsc::SyncSender<(i32, i32, f64, f64)>,
    pub(super) tile_rx: mpsc::Receiver<TileBundle>,
    pub(super) tile_loading: bool,
}

/// Find the finest IFD level (lowest index) where the pixel scale is at least
/// `min_scale_m` and a window of diameter `radius_m * 2` fits within `max_px`.
/// Falls back to the coarsest available IFD if no level meets both constraints.
pub(super) fn select_ifd(scales: &[f64], min_scale_m: f64, radius_m: f64, max_px: u32) -> usize {
    for (i, &scale) in scales.iter().enumerate() {
        let window_px = (radius_m * 2.0 / scale) as u32;
        if scale >= min_scale_m && window_px <= max_px {
            return i;
        }
    }
    scales.len().saturating_sub(1)
}

/// Persistent state for BEV two-tier mode.
pub(super) struct BevBaseState {
    pub(super) base: StreamingTier, // wide window, low resolution (IFD-2/1)
    pub(super) close: StreamingTier, // close window, 5 m/px (IFD-0)
    pub(super) fine: Option<StreamingTier>, // fine window, 1 m/px (1m tile IFD-0); None if no 1m tiles available
}

impl BevBaseState {
    /// Spawn all three background worker threads and return the populated state.
    /// Also performs a blocking initial close-tier load so the viewer starts with detail visible.
    /// `hm` is the already-loaded base heightmap; `scene` receives the initial upload.
    /// `close_ifd` / `base_ifd`: IFD levels selected by `select_ifd` for the two main tiers.
    /// `fine_ifd`: Some(ifd) → use `tile_path` at that IFD level for the 1m fine tier
    ///             (used when `tile_path` is itself a sub-5m tile and no separate 1m dir exists).
    pub(super) fn new(
        tile_path: &Path,
        lat_rad: f32,
        init_e: f64,
        init_n: f64,
        hm: &Arc<Heightmap>,
        tiles_1m_dir: Option<&Path>,
        scene: &mut GpuScene,
        close_ifd: usize,
        base_ifd: usize,
        fine_ifd: Option<usize>,
    ) -> Self {
        // ── base drift worker ──────────────────────────────────────────────────────────
        // loads BEV_BASE_RADIUS_M at the dynamically selected base_ifd each time
        // the camera drifts past the effective base threshold (see below)
        let tile_path_base = tile_path.to_path_buf();
        let (base_tx, base_worker_rx) = mpsc::sync_channel::<(f64, f64)>(1);
        let (base_worker_tx, base_rx) = mpsc::channel::<TierData>();
        let lat_rad_w = lat_rad;
        std::thread::spawn(move || {
            while let Ok((easting, northing)) = base_worker_rx.recv() {
                let Ok(hm) = extract_window(
                    &tile_path_base,
                    (easting, northing),
                    BEV_BASE_RADIUS_M,
                    base_ifd,
                ) else {
                    continue;
                };
                let hm = Arc::new(cap_to_gpu_limit(hm, easting, northing));
                let normals = terrain::compute_normals_vector_par(&hm);
                let (az, el) = sun_position(lat_rad_w, INIT_SIM_DAY, INIT_SIM_HOUR);
                let shadow = terrain::compute_shadow_vector_par_with_azimuth(&hm, az, el, 200.0);
                let cam_x = easting - hm.crs_origin_x;
                let cam_y = hm.crs_origin_y - northing;
                let ao = compute_ao_cropped(&hm, cam_x, cam_y);
                if base_worker_tx
                    .send(TierData {
                        hm,
                        normals,
                        shadow,
                        ao,
                        centre_e: easting,
                        centre_n: northing,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });

        // ── close-tier drift worker ────────────────────────────────────────────────────
        // loads BEV_5M_RADIUS_M at the dynamically selected close_ifd each time
        // the camera drifts past the effective close threshold (see below)
        let tile_path_5m = tile_path.to_path_buf();
        let (hm5m_tx, hm5m_worker_rx) = mpsc::sync_channel::<(f64, f64)>(1);
        let (hm5m_worker_tx, hm5m_rx) = mpsc::channel::<TierData>();
        let lat_rad_5m = lat_rad;
        std::thread::spawn(move || {
            while let Ok((easting, northing)) = hm5m_worker_rx.recv() {
                let Ok(hm5m) = extract_window(
                    &tile_path_5m,
                    (easting, northing),
                    BEV_5M_RADIUS_M,
                    close_ifd,
                ) else {
                    continue;
                };
                let hm5m = Arc::new(cap_to_gpu_limit(hm5m, easting, northing));
                let normals = terrain::compute_normals_vector_par(&hm5m);
                let (az, el) = sun_position(lat_rad_5m, INIT_SIM_DAY, INIT_SIM_HOUR);
                let shadow = terrain::compute_shadow_vector_par_with_azimuth(&hm5m, az, el, 200.0);
                // Use the *requested* centre, not the geometric window centre.
                // Near a tile edge the window is clipped, so its geometric centre drifts
                // away from the camera — triggering an infinite reload loop.
                if hm5m_worker_tx
                    .send(TierData {
                        hm: hm5m,
                        normals,
                        shadow,
                        ao: vec![],
                        centre_e: easting,
                        centre_n: northing,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });

        // ── blocking initial close-tier load ──────────────────────────────────────────
        // Loads synchronously so the viewer starts with close-range detail immediately
        // rather than waiting for the first drift threshold to fire.
        let mut last_5m_cx = 0.0_f64;
        let mut last_5m_cy = 0.0_f64;
        // Adjusted below once the close window is known; falls back to constant if load fails.
        let mut effective_close_threshold = BEV_5M_DRIFT_THRESHOLD_M;
        if let Ok(hm5m_init) =
            extract_window(tile_path, (init_e, init_n), BEV_5M_RADIUS_M, close_ifd)
        {
            let hm5m_init = cap_to_gpu_limit(hm5m_init, init_e, init_n);
            // When the GPU cap shrinks the window below BEV_5M_RADIUS_M (e.g. 1m tiles with no
            // overviews), keep the threshold at ≤ half the actual window half-extent so the
            // camera never exits the loaded window before a reload fires.
            let close_half_m = (hm5m_init.cols as f64 * hm5m_init.dx_meters)
                .min(hm5m_init.rows as f64 * hm5m_init.dy_meters)
                * 0.5;
            effective_close_threshold = BEV_5M_DRIFT_THRESHOLD_M.min(close_half_m * 0.5);
            let hm5m_init = Arc::new(hm5m_init);
            // tile-local offset of the 5m window's top-left corner within the base heightmap:
            // X = difference in left-edge eastings (both in same CRS, so direct subtraction)
            // Y = base top-northing minus 5m top-northing (flips axis: CRS Y↑ → tile Y↓)
            let origin_x = (hm5m_init.crs_origin_x - hm.crs_origin_x) as f32;
            let origin_y = (hm.crs_origin_y - hm5m_init.crs_origin_y) as f32;
            let normals5 = terrain::compute_normals_vector_par(&hm5m_init);
            let (az, el) = sun_position(lat_rad, INIT_SIM_DAY, INIT_SIM_HOUR);
            let shadow5 =
                terrain::compute_shadow_vector_par_with_azimuth(&hm5m_init, az, el, 200.0);
            last_5m_cx = init_e;
            last_5m_cy = init_n;
            println!(
                "close IFD-{} initial: {}×{} at {:.1}m/px",
                close_ifd, hm5m_init.cols, hm5m_init.rows, hm5m_init.dx_meters
            );
            scene.upload_hm5m(origin_x, origin_y, &hm5m_init, &normals5, &shadow5);
        } else {
            println!("warning: could not load initial 5m IFD-0 window");
        }

        // ── 1m fine-tier worker ────────────────────────────────────────────────────────
        // Two variants:
        //  a) tiles_1m_dir is Some → scan directory for CRS3035 tile files (BEV 1m tiles)
        //  b) fine_ifd is Some     → tile_path IS a sub-5m tile; use it directly at IFD-0
        let fine = match (tiles_1m_dir, fine_ifd) {
            // Variant b takes priority: when the main tile IS the fine source, cut directly
            // from it.  A provided tiles_1m_dir is irrelevant — it may point to a different
            // set of tiles that does not even contain the file currently being rendered.
            (_, Some(fine_ifd_val)) => {
                // Variant b: tile_path itself is the finest source (e.g. a 1m EPSG:3035 COG).
                // (easting, northing) arrives already in the tile's native CRS, so no
                // intermediate conversion is needed before calling extract_window.
                let (hm1m_tx, hm1m_worker_rx) = mpsc::sync_channel::<(f64, f64)>(1);
                let (hm1m_worker_tx, hm1m_rx) = mpsc::channel::<TierData>();
                let lat_rad_1m = lat_rad;
                let tile_path_fine = tile_path.to_path_buf();
                std::thread::spawn(move || {
                    while let Ok((easting, northing)) = hm1m_worker_rx.recv() {
                        let Ok(hm1m) = extract_window(
                            &tile_path_fine,
                            (easting, northing),
                            BEV_1M_RADIUS_M,
                            fine_ifd_val,
                        ) else {
                            continue;
                        };
                        let hm1m = Arc::new(cap_to_gpu_limit(hm1m, easting, northing));
                        let normals = terrain::compute_normals_vector_par(&hm1m);
                        let (az, el) = sun_position(lat_rad_1m, INIT_SIM_DAY, INIT_SIM_HOUR);
                        let shadow =
                            terrain::compute_shadow_vector_par_with_azimuth(&hm1m, az, el, 200.0);
                        if hm1m_worker_tx
                            .send(TierData {
                                hm: hm1m,
                                normals,
                                shadow,
                                ao: vec![],
                                centre_e: easting,
                                centre_n: northing,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                });
                Some(StreamingTier::new(
                    hm1m_tx,
                    hm1m_rx,
                    0.0,
                    0.0,
                    BEV_1M_DRIFT_THRESHOLD_M,
                ))
            }
            (Some(dir), None) => {
                // Variant a: directory of separate fine tile files.
                // Tile filenames encode their native CRS coordinates (e.g. CRS3035RES50000m…).
                // We probe the first found tile to read that CRS dynamically — no hardcoded EPSG.
                let (hm1m_tx, hm1m_worker_rx) = mpsc::sync_channel::<(f64, f64)>(1);
                let (hm1m_worker_tx, hm1m_rx) = mpsc::channel::<TierData>();
                let lat_rad_1m = lat_rad;
                let dir_1m = dir.to_path_buf();
                let base_proj4_1m = hm.crs_proj4.clone();
                std::thread::spawn(move || {
                    // Lazily read the tile CRS from the first tile found in the directory.
                    // This handles any CRS, not just EPSG:3035.
                    let mut tile_crs_proj4: Option<String> = None;
                    while let Ok((easting, northing)) = hm1m_worker_rx.recv() {
                        // probe once, then reuse; must find a CRS3035 tile specifically
                        if tile_crs_proj4.is_none() {
                            tile_crs_proj4 = find_first_1m_tile(&dir_1m)
                                .and_then(|p| dem_io::crs::tile_proj4(&p).ok());
                        }
                        let Some(ref tiles_crs) = tile_crs_proj4 else {
                            continue;
                        };
                        let Ok((lat, lon)) =
                            dem_io::crs::to_wgs84(easting, northing, &base_proj4_1m)
                        else {
                            continue;
                        };
                        let Ok((e_tile, n_tile)) = dem_io::crs::from_wgs84(lat, lon, tiles_crs)
                        else {
                            continue;
                        };
                        let tile_paths = find_1m_tiles(&dir_1m, e_tile, n_tile, BEV_1M_RADIUS_M);
                        if tile_paths.is_empty() {
                            continue;
                        }
                        let windows: Vec<_> = tile_paths
                            .iter()
                            .filter_map(|p| {
                                extract_window(p, (e_tile, n_tile), BEV_1M_RADIUS_M, 0).ok()
                            })
                            .collect();
                        if windows.is_empty() {
                            continue;
                        }
                        let raw1m = if windows.len() == 1 {
                            windows.into_iter().next().unwrap()
                        } else {
                            stitch_windows(windows, e_tile, n_tile, BEV_1M_RADIUS_M)
                        };
                        let hm1m = Arc::new(cap_to_gpu_limit(raw1m, e_tile, n_tile));
                        let normals = terrain::compute_normals_vector_par(&hm1m);
                        let (az, el) = sun_position(lat_rad_1m, INIT_SIM_DAY, INIT_SIM_HOUR);
                        let shadow =
                            terrain::compute_shadow_vector_par_with_azimuth(&hm1m, az, el, 200.0);
                        if hm1m_worker_tx
                            .send(TierData {
                                hm: hm1m,
                                normals,
                                shadow,
                                ao: vec![],
                                centre_e: easting,
                                centre_n: northing,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                });
                Some(StreamingTier::new(
                    hm1m_tx,
                    hm1m_rx,
                    0.0,
                    0.0,
                    BEV_1M_DRIFT_THRESHOLD_M,
                ))
            }
            (None, None) => None,
        };

        // Base drift threshold: cap to half the actual window half-extent so that the camera
        // always stays inside the loaded window between reloads.  For large-overview tiles
        // the window >> BEV_BASE_RADIUS_M and the constant wins; for GPU-capped tiles
        // (e.g. 1m NZ LiDAR, 8192 px = 8 km) the derived value is much smaller.
        let base_half_m = (hm.cols as f64 * hm.dx_meters).min(hm.rows as f64 * hm.dy_meters) * 0.5;
        let effective_base_threshold = BEV_BASE_DRIFT_THRESHOLD_M.min(base_half_m * 0.5);

        BevBaseState {
            base: StreamingTier::new(base_tx, base_rx, init_e, init_n, effective_base_threshold),
            close: StreamingTier::new(
                hm5m_tx,
                hm5m_rx,
                last_5m_cx,
                last_5m_cy,
                effective_close_threshold,
            ),
            fine,
        }
    }
}

impl Glo30State {
    /// Spawn the background tile-slide worker and return the initial state centred on `(cam_lat, cam_lon)`.
    pub(super) fn new(tiles_dir: &Path, lat_rad: f32, cam_lat: f64, cam_lon: f64) -> Self {
        let tiles_dir_w = tiles_dir.to_path_buf();
        let (tile_tx, tile_worker_rx) = mpsc::sync_channel::<(i32, i32, f64, f64)>(1);
        let (tile_worker_tx, tile_rx) = mpsc::channel::<TileBundle>();
        let lat_rad_w = lat_rad;
        std::thread::spawn(move || {
            while let Ok((new_lat, new_lon, cam_lat_w, cam_lon_w)) = tile_worker_rx.recv() {
                let hm = Arc::new(load_grid(&tiles_dir_w, new_lat, new_lon, |p| {
                    parse_geotiff_auto(p).ok()
                }));
                let normals = terrain::compute_normals_vector_par(&hm);
                let (az, el) = sun_position(lat_rad_w, INIT_SIM_DAY, INIT_SIM_HOUR);
                let shadow = terrain::compute_shadow_vector_par_with_azimuth(&hm, az, el, 200.0);
                let cam_x = (cam_lon_w - hm.crs_origin_x) / hm.dx_deg * hm.dx_meters;
                let cam_y = (hm.crs_origin_y - cam_lat_w) / hm.dy_deg.abs() * hm.dy_meters;
                let ao = compute_ao_cropped(&hm, cam_x, cam_y);
                let bundle = TileBundle {
                    hm,
                    normals,
                    shadow,
                    ao,
                    centre_lat: new_lat,
                    centre_lon: new_lon,
                    cam_lat: cam_lat_w,
                    cam_lon: cam_lon_w,
                };
                if tile_worker_tx.send(bundle).is_err() {
                    break;
                }
            }
        });
        Glo30State {
            centre_lat: cam_lat.floor() as i32,
            centre_lon: cam_lon.floor() as i32,
            tile_tx,
            tile_rx,
            tile_loading: false,
        }
    }
}
