use std::path::Path;
use std::sync::{Arc, mpsc};

use dem_io::{Heightmap, crop, extract_window, load_grid, parse_geotiff, stitch_windows};
use render_gpu::GpuScene;
use terrain::{NormalMap, ShadowMask};

use super::geo::{laea_epsg3035, lcc_epsg31287_inverse, sun_position};
use super::scene_init::{INIT_SIM_DAY, INIT_SIM_HOUR, compute_ao_cropped};

// BEV COG (DGM_R5.tif) tier geometry.
// IFD-0 = 5 m/px  (full resolution, always present)
// IFD-1 ≈ 10 m/px (first overview, may be absent)
// IFD-2 ≈ 20 m/px (second overview, may be absent — preferred for the base window)
// Changing BEV_BASE_RADIUS_M or BEV_BASE_IFD requires updating prepare_scene too.
pub(super) const BEV_BASE_IFD: usize = 3;
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
/// whose 50 km bounds overlap the window [e3035±radius_m) × [n3035±radius_m).
pub(super) fn find_1m_tiles(
    dir: &Path,
    e3035: f64,
    n3035: f64,
    radius_m: f64,
) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let Ok(walker) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in walker.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(find_1m_tiles(&path, e3035, n3035, radius_m));
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
        if tile_e < e3035 + radius_m
            && tile_e + BEV_TILE_SIZE_M > e3035 - radius_m
            && tile_n < n3035 + radius_m
            && tile_n + BEV_TILE_SIZE_M > n3035 - radius_m
        {
            found.push(path);
        }
    }
    found
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

/// Persistent state for BEV two-tier mode.
pub(super) struct BevBaseState {
    pub(super) base: StreamingTier, // wide window, low resolution (IFD-2/1)
    pub(super) close: StreamingTier, // close window, 5 m/px (IFD-0)
    pub(super) fine: Option<StreamingTier>, // fine window, 1 m/px (1m tile IFD-0); None if no 1m tiles available
}

/// Crop a heightmap to fit within the GPU texture dimension limit.
/// Trims equally from both sides so the loaded area stays centred.
fn crop_to_gpu_limit(hm: Heightmap, max_dim: usize) -> Heightmap {
    if hm.cols <= max_dim && hm.rows <= max_dim {
        return hm;
    }
    let crop_cols = hm.cols.min(max_dim);
    let crop_rows = hm.rows.min(max_dim);
    let col_start = (hm.cols - crop_cols) / 2;
    let row_start = (hm.rows - crop_rows) / 2;
    crop(&hm, row_start, col_start, crop_rows, crop_cols)
}

impl BevBaseState {
    /// Spawn all three background worker threads and return the populated state.
    /// Also performs a blocking initial 5m load so the viewer starts with close detail visible.
    /// `hm` is the already-loaded base heightmap; `scene` receives the initial 5m upload.
    pub(super) fn new(
        tile_path: &Path,
        lat_rad: f32,
        init_e: f64,
        init_n: f64,
        hm: &Arc<Heightmap>,
        tiles_1m_dir: Option<&Path>,
        scene: &mut GpuScene,
    ) -> Self {
        // ── base drift worker ──────────────────────────────────────────────────────────
        // base drift worker: loads a BEV_BASE_RADIUS_M window at BEV_BASE_IFD each time
        // the camera drifts BEV_BASE_DRIFT_THRESHOLD_M from the last window centre
        let tile_path_base = tile_path.to_path_buf();
        let (base_tx, base_worker_rx) = mpsc::sync_channel::<(f64, f64)>(1);
        let (base_worker_tx, base_rx) = mpsc::channel::<TierData>();
        let lat_rad_w = lat_rad;
        std::thread::spawn(move || {
            while let Ok((easting, northing)) = base_worker_rx.recv() {
                // try BEV_BASE_IFD first; fall back to IFD-1 if that overview is absent
                let hm_result = extract_window(
                    &tile_path_base,
                    (easting, northing),
                    BEV_BASE_RADIUS_M,
                    BEV_BASE_IFD,
                    31287,
                )
                .or_else(|_| {
                    extract_window(
                        &tile_path_base,
                        (easting, northing),
                        BEV_BASE_RADIUS_M,
                        1,
                        31287,
                    )
                });
                let Ok(hm) = hm_result else { continue };
                let hm = Arc::new(hm);
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

        let max_tex_dim = scene.max_texture_dim();

        // ── 5m close-tier drift worker ─────────────────────────────────────────────────
        // 5m close-tier drift worker: loads a BEV_5M_RADIUS_M window at IFD-0 (5 m/px)
        // each time the camera drifts BEV_5M_DRIFT_THRESHOLD_M from the last window centre.
        // IFD-0 is always present so no fallback is needed here.
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
                    0,
                    31287,
                ) else {
                    continue;
                };
                let hm5m = crop_to_gpu_limit(hm5m, max_tex_dim);
                let hm5m = Arc::new(hm5m);
                let normals = terrain::compute_normals_vector_par(&hm5m);
                let (az, el) = sun_position(lat_rad_5m, INIT_SIM_DAY, INIT_SIM_HOUR);
                let shadow = terrain::compute_shadow_vector_par_with_azimuth(&hm5m, az, el, 200.0);
                // worker computes the window centre so try_recv() can update last_cx/cy generically
                let centre_e = hm5m.crs_origin_x + hm5m.cols as f64 * hm5m.dx_meters * 0.5;
                let centre_n = hm5m.crs_origin_y - hm5m.rows as f64 * hm5m.dy_meters * 0.5;
                if hm5m_worker_tx
                    .send(TierData {
                        hm: hm5m,
                        normals,
                        shadow,
                        ao: vec![],
                        centre_e,
                        centre_n,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });

        // ── blocking initial 5m load ───────────────────────────────────────────────────
        // Loads synchronously so the viewer starts with close-range detail immediately
        // rather than waiting for the first drift threshold to fire.
        let mut last_5m_cx = 0.0_f64;
        let mut last_5m_cy = 0.0_f64;
        if let Ok(hm5m_init) =
            extract_window(tile_path, (init_e, init_n), BEV_5M_RADIUS_M, 0, 31287)
        {
            let hm5m_init = Arc::new(crop_to_gpu_limit(hm5m_init, max_tex_dim));
            // tile-local offset of the 5m window's top-left corner within the base heightmap:
            // X = difference in left-edge eastings (both in same CRS, so direct subtraction)
            // Y = base top-northing minus 5m top-northing (flips axis: CRS Y↑ → tile Y↓)
            let origin_x = (hm5m_init.crs_origin_x - hm.crs_origin_x) as f32;
            let origin_y = (hm.crs_origin_y - hm5m_init.crs_origin_y) as f32;
            let normals5 = terrain::compute_normals_vector_par(&hm5m_init);
            let (az, el) = sun_position(lat_rad, INIT_SIM_DAY, INIT_SIM_HOUR);
            let shadow5 =
                terrain::compute_shadow_vector_par_with_azimuth(&hm5m_init, az, el, 200.0);
            last_5m_cx = hm5m_init.crs_origin_x + hm5m_init.cols as f64 * hm5m_init.dx_meters * 0.5;
            last_5m_cy = hm5m_init.crs_origin_y - hm5m_init.rows as f64 * hm5m_init.dy_meters * 0.5;
            println!(
                "5m IFD-0 initial: {}×{} at {:.1}m/px",
                hm5m_init.cols, hm5m_init.rows, hm5m_init.dx_meters
            );
            scene.upload_hm5m(origin_x, origin_y, &hm5m_init, &normals5, &shadow5);
        } else {
            println!("warning: could not load initial 5m IFD-0 window");
        }

        // ── 1m fine-tier worker ────────────────────────────────────────────────────────
        // 1m fine-tier worker: loads a BEV_1M_RADIUS_M window from 1m tiles (EPSG:3035)
        // each time the camera drifts BEV_1M_DRIFT_THRESHOLD_M from the last window centre.
        // Tiles are discovered dynamically on each reload; multiple tiles are stitched
        // together when the window straddles a tile boundary at easting 4450000.
        let fine = tiles_1m_dir.map(|dir| {
            let (hm1m_tx, hm1m_worker_rx) = mpsc::sync_channel::<(f64, f64)>(1);
            let (hm1m_worker_tx, hm1m_rx) = mpsc::channel::<TierData>();
            let lat_rad_1m = lat_rad;
            let dir_1m = dir.to_path_buf();
            std::thread::spawn(move || {
                while let Ok((easting, northing)) = hm1m_worker_rx.recv() {
                    let (lat, lon) = lcc_epsg31287_inverse(easting, northing);
                    let (e3035, n3035) = laea_epsg3035(lat, lon);
                    let tile_paths = find_1m_tiles(&dir_1m, e3035, n3035, BEV_1M_RADIUS_M);
                    if tile_paths.is_empty() {
                        continue;
                    }
                    let windows: Vec<_> = tile_paths
                        .iter()
                        .filter_map(|p| {
                            extract_window(p, (e3035, n3035), BEV_1M_RADIUS_M, 0, 3035).ok()
                        })
                        .collect();
                    if windows.is_empty() {
                        continue;
                    }
                    let hm1m = Arc::new(if windows.len() == 1 {
                        windows.into_iter().next().unwrap()
                    } else {
                        stitch_windows(windows, e3035, n3035, BEV_1M_RADIUS_M)
                    });
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
            StreamingTier::new(hm1m_tx, hm1m_rx, 0.0, 0.0, BEV_1M_DRIFT_THRESHOLD_M)
        });

        BevBaseState {
            base: StreamingTier::new(base_tx, base_rx, init_e, init_n, BEV_BASE_DRIFT_THRESHOLD_M),
            close: StreamingTier::new(
                hm5m_tx,
                hm5m_rx,
                last_5m_cx,
                last_5m_cy,
                BEV_5M_DRIFT_THRESHOLD_M,
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
                    parse_geotiff(p).ok()
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
