use std::sync::{Arc, mpsc};

use dem_io::{Heightmap, crop, extract_window, stitch_windows, stitch_windows_geographic};
use render_gpu::GpuScene;
use terrain::{NormalMap, ShadowMask};

use super::geo::sun_position;
use super::scene_init::{INIT_SIM_DAY, INIT_SIM_HOUR, compute_ao_cropped};
use crate::consts::{GPU_SAFE_PX, M_PER_DEG};

// BEV COG tier geometry.
pub(super) const BEV_BASE_RADIUS_M: f64 = 90_000.0;
pub(super) const BEV_BASE_DRIFT_THRESHOLD_M: f64 = 30_000.0;
pub(super) const BEV_5M_RADIUS_M: f64 = 20_000.0;
pub(super) const BEV_5M_DRIFT_THRESHOLD_M: f64 = 3_000.0;
pub(super) const BEV_1M_RADIUS_M: f64 = 3_500.0;
pub(super) const BEV_1M_DRIFT_THRESHOLD_M: f64 = 1_000.0;

/// Crop a heightmap to at most `GPU_SAFE_PX × GPU_SAFE_PX` pixels centered on
/// `(centre_e, centre_n)` (CRS-native: easting/northing for projected, lon/lat for geographic).
/// No-op when the heightmap already fits.
pub(super) fn cap_to_gpu_limit(hm: Heightmap, centre_e: f64, centre_n: f64) -> Heightmap {
    if hm.cols <= GPU_SAFE_PX && hm.rows <= GPU_SAFE_PX {
        return hm;
    }
    // For geographic tiles dx_meters stores deg/px, not m/px — use dx_deg / dy_deg for
    // pixel position.  For projected tiles dx_deg == 0.0.
    let (px_per_unit_x, px_per_unit_y) = if hm.dx_deg != 0.0 {
        (1.0 / hm.dx_deg, 1.0 / hm.dy_deg)
    } else {
        (1.0 / hm.dx_meters, 1.0 / hm.dy_meters)
    };
    let cam_col =
        ((centre_e - hm.crs_origin_x) * px_per_unit_x).clamp(0.0, (hm.cols - 1) as f64) as usize;
    let cam_row =
        ((hm.crs_origin_y - centre_n) * px_per_unit_y).clamp(0.0, (hm.rows - 1) as f64) as usize;
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
/// `centre_lat`/`centre_lon` are WGS84 degrees of the loaded window centre.
pub(super) struct TierData {
    pub(super) hm: Arc<Heightmap>,
    pub(super) normals: NormalMap,
    pub(super) shadow: ShadowMask,
    pub(super) ao: Vec<f32>,
    pub(super) centre_lat: f64,
    pub(super) centre_lon: f64,
}

/// Per-tier channel state and drift-detection bookkeeping.
///
/// `last_cx`/`last_cy` store WGS84 (lat, lon) in degrees.
/// `drift_threshold_m` is stored in degrees (metres / M_PER_DEG).
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
                self.last_cx = data.centre_lat;
                self.last_cy = data.centre_lon;
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

    /// Update the drift threshold to match the actual loaded window half-extent.
    /// Called after a base tier reload so the threshold reflects the real window size
    /// rather than the initial (potentially much smaller) estimate.
    pub(super) fn update_threshold(&mut self, new_threshold_m: f64) {
        self.drift_threshold_m = new_threshold_m;
    }
}

/// Find the finest IFD level where scale ≥ `min_scale_m` and window fits in `max_px`.
pub(super) fn select_ifd(scales: &[f64], min_scale_m: f64, radius_m: f64, max_px: u32) -> usize {
    for (i, &scale) in scales.iter().enumerate() {
        let window_px = (radius_m * 2.0 / scale) as u32;
        if scale >= min_scale_m && window_px <= max_px {
            return i;
        }
    }
    scales.len().saturating_sub(1)
}

/// Persistent state for BEV multi-tier streaming mode.
pub(super) struct BevBaseState {
    pub(super) base: StreamingTier, // wide window, low resolution (IFD-2/1)
    pub(super) close: StreamingTier, // close window, 5 m/px (IFD-0)
    pub(super) fine: Option<StreamingTier>, // fine window, 1 m/px (1m tile IFD-0); None if no 1m tiles available
}

impl BevBaseState {
    /// Spawn all three tier workers and return the populated state.
    ///
    /// Works for both demo view (3 TileIndex from config, `TileEntry.ifd = 0`) and single-file
    /// mode (1-entry TileIndex per tier with pre-selected IFD).  All workers communicate in
    /// WGS84 `(lat, lon)` and convert to each tile's native CRS independently.
    ///
    /// `hm` is the already-loaded base heightmap; `scene` receives a synchronous initial
    /// close-tier upload so the viewer starts with close-range detail visible immediately.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        fine_index: Arc<super::tile_index::TileIndex>,
        close_index: Arc<super::tile_index::TileIndex>,
        base_index: Arc<super::tile_index::TileIndex>,
        cam_lat: f64,
        cam_lon: f64,
        lat_rad: f32,
        hm: &Arc<Heightmap>,
        scene: &mut GpuScene,
    ) -> Self {
        use super::tile_index::tiles_overlapping_wgs84;

        // ── base worker ────────────────────────────────────────────────────────────────
        let (base_tx, base_worker_rx) = mpsc::sync_channel::<(f64, f64)>(1);
        let (base_worker_tx, base_rx) = mpsc::channel::<TierData>();
        let base_idx = Arc::clone(&base_index);
        let lat_rad_b = lat_rad;
        std::thread::spawn(move || {
            while let Ok((lat, lon)) = base_worker_rx.recv() {
                let radius_deg_lat = BEV_BASE_RADIUS_M / M_PER_DEG;
                let radius_deg_lon = BEV_BASE_RADIUS_M / (M_PER_DEG * lat.to_radians().cos());
                let overlapping = tiles_overlapping_wgs84(&base_idx, lat, lon, BEV_BASE_RADIUS_M);
                if overlapping.is_empty() {
                    continue;
                }
                // Convert camera to the first entry's CRS for AO and cap_to_gpu_limit.
                let first = &base_idx[overlapping[0]];
                let Ok((cam_cx, cam_cy)) = dem_io::crs::from_wgs84(lat, lon, &first.crs_proj4)
                else {
                    continue;
                };
                let is_geo = dem_io::crs::is_geographic(&first.crs_proj4);
                let windows: Vec<_> = overlapping
                    .iter()
                    .filter_map(|&i| {
                        let e = &base_idx[i];
                        let Ok((cx, cy)) = dem_io::crs::from_wgs84(lat, lon, &e.crs_proj4) else {
                            return None;
                        };
                        let radius = if is_geo {
                            radius_deg_lon.max(radius_deg_lat)
                        } else {
                            BEV_BASE_RADIUS_M
                        };
                        extract_window(&e.path, (cx, cy), radius, e.ifd).ok()
                    })
                    .collect();
                if windows.is_empty() {
                    continue;
                }
                let raw = if windows.len() == 1 {
                    windows.into_iter().next().unwrap()
                } else {
                    stitch_windows_geographic(windows, lon, lat, radius_deg_lon, radius_deg_lat)
                };
                let hm = Arc::new(cap_to_gpu_limit(raw, cam_cx, cam_cy));
                let normals = terrain::compute_normals_vector_par(&hm);
                let (az, el) = sun_position(lat_rad_b, INIT_SIM_DAY, INIT_SIM_HOUR);
                let shadow = terrain::compute_shadow_vector_par_with_azimuth(&hm, az, el, 200.0);
                let (cam_x, cam_y) = if dem_io::crs::is_geographic(&hm.crs_proj4) {
                    let px = (lon - hm.crs_origin_x) / hm.dx_deg;
                    let py = (hm.crs_origin_y - lat) / hm.dy_deg.abs();
                    (px * hm.dx_meters, py * hm.dy_meters)
                } else {
                    (cam_cx - hm.crs_origin_x, hm.crs_origin_y - cam_cy)
                };
                let ao = compute_ao_cropped(&hm, cam_x, cam_y);
                if base_worker_tx
                    .send(TierData {
                        hm,
                        normals,
                        shadow,
                        ao,
                        centre_lat: lat,
                        centre_lon: lon,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });

        // ── close worker ───────────────────────────────────────────────────────────────
        let (hm5m_tx, hm5m_worker_rx) = mpsc::sync_channel::<(f64, f64)>(1);
        let (hm5m_worker_tx, hm5m_rx) = mpsc::channel::<TierData>();
        let close_idx = Arc::clone(&close_index);
        let lat_rad_5m = lat_rad;
        std::thread::spawn(move || {
            while let Ok((lat, lon)) = hm5m_worker_rx.recv() {
                let overlapping = tiles_overlapping_wgs84(&close_idx, lat, lon, BEV_5M_RADIUS_M);
                if overlapping.is_empty() {
                    continue;
                }
                let entry = &close_idx[overlapping[0]];
                let Ok((cx, cy)) = dem_io::crs::from_wgs84(lat, lon, &entry.crs_proj4) else {
                    continue;
                };
                let Ok(hm5m) = extract_window(&entry.path, (cx, cy), BEV_5M_RADIUS_M, entry.ifd)
                else {
                    continue;
                };
                let hm5m = Arc::new(cap_to_gpu_limit(hm5m, cx, cy));
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
                        centre_lat: lat,
                        centre_lon: lon,
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
        let mut last_5m_lat = 0.0_f64;
        let mut last_5m_lon = 0.0_f64;
        let mut effective_close_threshold = BEV_5M_DRIFT_THRESHOLD_M;
        let overlapping_close =
            tiles_overlapping_wgs84(&close_index, cam_lat, cam_lon, BEV_5M_RADIUS_M);
        if let Some(&ci) = overlapping_close.first() {
            let entry = &close_index[ci];
            if let Ok((cx, cy)) = dem_io::crs::from_wgs84(cam_lat, cam_lon, &entry.crs_proj4) {
                if let Ok(hm5m_init) =
                    extract_window(&entry.path, (cx, cy), BEV_5M_RADIUS_M, entry.ifd)
                {
                    let hm5m_init = cap_to_gpu_limit(hm5m_init, cx, cy);
                    // When the GPU cap shrinks the window below BEV_5M_RADIUS_M (e.g. 1m tiles with no
                    // overviews), keep the threshold at ≤ half the actual window half-extent so the
                    // camera never exits the loaded window before a reload fires.
                    let close_half_m = (hm5m_init.cols as f64 * hm5m_init.dx_meters)
                        .min(hm5m_init.rows as f64 * hm5m_init.dy_meters)
                        * 0.5;
                    effective_close_threshold = BEV_5M_DRIFT_THRESHOLD_M.min(close_half_m * 0.5);
                    let (origin_x, origin_y) = cross_crs_world_origin(&hm5m_init, hm);
                    let hm5m_init = Arc::new(hm5m_init);
                    let normals5 = terrain::compute_normals_vector_par(&hm5m_init);
                    let (az, el) = sun_position(lat_rad, INIT_SIM_DAY, INIT_SIM_HOUR);
                    let shadow5 =
                        terrain::compute_shadow_vector_par_with_azimuth(&hm5m_init, az, el, 200.0);
                    last_5m_lat = cam_lat;
                    last_5m_lon = cam_lon;
                    println!(
                        "close IFD-{} initial: {}×{} at {:.1}m/px",
                        entry.ifd, hm5m_init.cols, hm5m_init.rows, hm5m_init.dx_meters
                    );
                    scene.upload_hm5m(origin_x, origin_y, &hm5m_init, &normals5, &shadow5);
                }
            }
        }

        // ── fine worker ────────────────────────────────────────────────────────────────
        let fine = if fine_index.is_empty() {
            None
        } else {
            let (hm1m_tx, hm1m_worker_rx) = mpsc::sync_channel::<(f64, f64)>(1);
            let (hm1m_worker_tx, hm1m_rx) = mpsc::channel::<TierData>();
            let fine_idx = Arc::clone(&fine_index);
            let lat_rad_1m = lat_rad;
            std::thread::spawn(move || {
                while let Ok((lat, lon)) = hm1m_worker_rx.recv() {
                    let overlapping = tiles_overlapping_wgs84(&fine_idx, lat, lon, BEV_1M_RADIUS_M);
                    if overlapping.is_empty() {
                        continue;
                    }
                    let entry = &fine_idx[overlapping[0]];
                    let Ok((e_tile, n_tile)) = dem_io::crs::from_wgs84(lat, lon, &entry.crs_proj4)
                    else {
                        continue;
                    };
                    let windows: Vec<_> = overlapping
                        .iter()
                        .filter_map(|&i| {
                            let e = &fine_idx[i];
                            let Ok((et, nt)) = dem_io::crs::from_wgs84(lat, lon, &e.crs_proj4)
                            else {
                                return None;
                            };
                            extract_window(&e.path, (et, nt), BEV_1M_RADIUS_M, e.ifd).ok()
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
                            centre_lat: lat,
                            centre_lon: lon,
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
                BEV_1M_DRIFT_THRESHOLD_M / M_PER_DEG,
            ))
        };

        // Base drift threshold: cap to half the actual window half-extent so that the camera
        // always stays inside the loaded window between reloads.  For large-overview tiles
        // the window >> BEV_BASE_RADIUS_M and the constant wins; for GPU-capped tiles
        // (e.g. 1m NZ LiDAR, 8192 px = 8 km) the derived value is much smaller.
        let base_half_m = (hm.cols as f64 * hm.dx_meters).min(hm.rows as f64 * hm.dy_meters) * 0.5;
        let effective_base_threshold = BEV_BASE_DRIFT_THRESHOLD_M.min(base_half_m * 0.5);
        let base_drift_deg = effective_base_threshold / M_PER_DEG;

        BevBaseState {
            base: StreamingTier::new(base_tx, base_rx, cam_lat, cam_lon, base_drift_deg),
            close: StreamingTier::new(
                hm5m_tx,
                hm5m_rx,
                last_5m_lat,
                last_5m_lon,
                effective_close_threshold / M_PER_DEG,
            ),
            fine,
        }
    }
}

/// Compute the tile-local position of `hm`'s top-left corner in `base_hm`'s world frame
/// (metres from base_hm's top-left, X right, Y down). Routes through WGS84 for any CRS pair.
pub(super) fn cross_crs_world_origin(hm: &Heightmap, base_hm: &Heightmap) -> (f32, f32) {
    if hm.crs_proj4 == base_hm.crs_proj4 {
        return (
            (hm.crs_origin_x - base_hm.crs_origin_x) as f32,
            (base_hm.crs_origin_y - hm.crs_origin_y) as f32,
        );
    }
    let Ok((lat, lon)) = dem_io::crs::to_wgs84(hm.crs_origin_x, hm.crs_origin_y, &hm.crs_proj4)
    else {
        return (0.0, 0.0);
    };
    if dem_io::crs::is_geographic(&base_hm.crs_proj4) {
        // base is geographic; tile-local metres = pixel * dx_meters (dx_meters = m/px after stitch)
        let px = (lon - base_hm.crs_origin_x) / base_hm.dx_deg;
        let py = (base_hm.crs_origin_y - lat) / base_hm.dy_deg;
        (
            (px * base_hm.dx_meters) as f32,
            (py * base_hm.dy_meters) as f32,
        )
    } else {
        let Ok((e, n)) = dem_io::crs::from_wgs84(lat, lon, &base_hm.crs_proj4) else {
            return (0.0, 0.0);
        };
        (
            (e - base_hm.crs_origin_x) as f32,
            (base_hm.crs_origin_y - n) as f32,
        )
    }
}
