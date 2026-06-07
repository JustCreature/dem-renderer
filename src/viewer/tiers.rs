use std::sync::{Arc, mpsc};

use dem_io::{Heightmap, crop, extract_window, stitch_windows, stitch_windows_geographic};
use render_gpu::GpuScene;
use terrain::ShadowMask;

use render_gpu::VramClass;

use super::geo::sun_position;
use super::scene_init::{INIT_SIM_DAY, INIT_SIM_HOUR, compute_ao_cropped};
use crate::consts::{GPU_SAFE_PX, M_PER_DEG};

/// Resolved tier geometry for the active VRAM class.
///
/// `fine.radius_m == 0.0` is the sentinel for "don't spawn the fine tier
/// worker"; the BevBaseState stores `fine: None` in that case and the viewer's
/// reload loop short-circuits on it. None of the shipped presets set it to
/// zero — the Low preset keeps a tiny fine window (1 km radius / 300 m drift)
/// so the user still sees 1 m detail right around the camera. The runtime OOM
/// handler mutates it to 0.0 if the actual GPU pressure turns out to be
/// tighter than the preset assumed.
#[derive(Clone, Copy, Debug)]
pub(super) struct TierRadii {
    pub(super) base_radius_m: f64,
    pub(super) base_drift_m: f64,
    pub(super) close_radius_m: f64,
    pub(super) close_drift_m: f64,
    pub(super) fine_radius_m: f64,
    pub(super) fine_drift_m: f64,
}

/// Map a VRAM class to tier radii / drift thresholds.
///
/// Memory math (Tirol demo, with the drop-first eager-dealloc reload cycle):
///
/// | preset | base | close | fine | steady mem | reload peak |
/// |---|---|---|---|---|---|
/// | High  | 90 km / 30 km drift | 20 km / 3 km drift | 3.5 km / 1 km drift | ~2.6 GB | ~2.6 GB |
/// | Mid   | 70 km / 23 km drift | 14 km / 2 km drift | 2.5 km / 800 m drift | ~1.7 GB | ~1.7 GB |
/// | Low   | 50 km / 17 km drift | 8 km / 1.5 km drift | 1 km / 300 m drift  | ~0.7 GB | ~0.7 GB |
///
/// The "High" row is the project's original hardcoded geometry; it stays the
/// reference for systems with no memory pressure (Apple Silicon, 8 GB+ discrete).
///
/// Low's fine tier loads ~2000×2000 R32Float (≈ 48 MB across hm + normal +
/// shadow) which is cheap enough to fit on a 4 GB card and gives a small island
/// of 1 m detail right around the camera. The drift threshold is tighter (300 m
/// vs the High preset's 1 km) because the smaller window means the camera
/// leaves it sooner — but the source is local IO + CPU work, not GPU memory,
/// so frequent reloads are fine.
pub(super) fn tier_radii(class: VramClass) -> TierRadii {
    match class {
        VramClass::High => TierRadii {
            base_radius_m: 90_000.0,
            base_drift_m: 30_000.0,
            close_radius_m: 20_000.0,
            close_drift_m: 3_000.0,
            fine_radius_m: 3_500.0,
            fine_drift_m: 1_000.0,
        },
        VramClass::Mid => TierRadii {
            base_radius_m: 70_000.0,
            base_drift_m: 23_000.0,
            close_radius_m: 14_000.0,
            close_drift_m: 2_000.0,
            fine_radius_m: 2_500.0,
            fine_drift_m: 800.0,
        },
        VramClass::Low => TierRadii {
            base_radius_m: 50_000.0,
            base_drift_m: 17_000.0,
            close_radius_m: 8_000.0,
            close_drift_m: 1_500.0,
            // Tiny fine window — still useful at low altitudes, ~48 MB of GPU
            // memory total. The runtime OOM handler may zero this on pressure.
            fine_radius_m: 1_000.0,
            fine_drift_m: 300.0,
        },
    }
}

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
/// All `gpu_*` fields are pre-converted to GPU-ready byte layouts on the worker thread so
/// the main thread only needs to call `write_texture`/`write_buffer` — no blocking CPU work.
pub(super) struct TierData {
    pub(super) hm: Arc<Heightmap>,
    pub(super) shadow: ShadowMask,
    pub(super) centre_lat: f64,
    pub(super) centre_lon: f64,
    /// Rg16Snorm bytes (4 bytes/pixel): normal texture for 5m/1m tiers.
    pub(super) gpu_normals_rg16: Vec<u8>,
    /// u32-packed normal bytes (4 bytes/pixel): storage buffer for base tier.
    pub(super) gpu_normals_u32: Vec<u8>,
    /// R16Float bytes (2 bytes/pixel): heightmap texture for base tier.
    pub(super) gpu_hm_f16: Vec<u8>,
    /// Pre-generated mip levels (width, height, bytes) for base tier heightmap.
    pub(super) gpu_hm_mips: Vec<(u32, u32, Vec<u8>)>,
    /// R8Unorm bytes (1 byte/pixel): AO texture for base tier.
    pub(super) gpu_ao_u8: Vec<u8>,
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
        radii: TierRadii,
        hm: &Arc<Heightmap>,
        scene: &mut GpuScene,
    ) -> Self {
        use super::tile_index::tiles_overlapping_wgs84;

        let base_radius_m = radii.base_radius_m;
        let close_radius_m = radii.close_radius_m;
        let fine_radius_m = radii.fine_radius_m;

        // ── base worker ────────────────────────────────────────────────────────────────
        let (base_tx, base_worker_rx) = mpsc::sync_channel::<(f64, f64)>(1);
        let (base_worker_tx, base_rx) = mpsc::channel::<TierData>();
        let base_idx = Arc::clone(&base_index);
        let lat_rad_b = lat_rad;
        std::thread::spawn(move || {
            while let Ok((lat, lon)) = base_worker_rx.recv() {
                let radius_deg_lat = base_radius_m / M_PER_DEG;
                let radius_deg_lon = base_radius_m / (M_PER_DEG * lat.to_radians().cos());
                let overlapping = tiles_overlapping_wgs84(&base_idx, lat, lon, base_radius_m);
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
                            base_radius_m
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
                let mut hm = cap_to_gpu_limit(raw, cam_cx, cam_cy);
                dem_io::clamp_nodata_to_sea(&mut hm);
                let hm = Arc::new(hm);
                let normals = terrain::compute_normals_vector_par(&hm);
                let (az, el) = sun_position(lat_rad_b, INIT_SIM_DAY, INIT_SIM_HOUR);
                let shadow = terrain::compute_shadow_vector_par_with_azimuth(&hm, az, el, 200.0);
                let (cam_x, cam_y) = if dem_io::crs::is_geographic(&hm.crs_proj4) {
                    let dx_m = hm.dx_deg * M_PER_DEG * lat.to_radians().cos();
                    let dy_m = hm.dy_deg.abs() * M_PER_DEG;
                    let px = (lon - hm.crs_origin_x) / hm.dx_deg;
                    let py = (hm.crs_origin_y - lat) / hm.dy_deg.abs();
                    (px * dx_m, py * dy_m)
                } else {
                    (cam_cx - hm.crs_origin_x, hm.crs_origin_y - cam_cy)
                };
                let ao = compute_ao_cropped(&hm, cam_x, cam_y);
                let gpu_hm_f16 = render_gpu::hm_to_f16_bytes(&hm.data);
                let gpu_hm_mips = render_gpu::gen_hm_mip_bytes(&gpu_hm_f16, hm.cols, hm.rows);
                let gpu_normals_u32 = render_gpu::pack_normals_u32_bytes(&normals.nx, &normals.ny);
                let gpu_ao_u8 = render_gpu::pack_ao_u8(&ao);
                if base_worker_tx
                    .send(TierData {
                        hm,
                        shadow,
                        centre_lat: lat,
                        centre_lon: lon,
                        gpu_normals_rg16: vec![],
                        gpu_normals_u32,
                        gpu_hm_f16,
                        gpu_hm_mips,
                        gpu_ao_u8,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });

        // ── close worker ───────────────────────────────────────────────────────────────
        // Shared slot: close worker writes its latest filled hm; fine worker reads it to
        // fill 1m NODATA from the 5m source.
        let recent_5m: Arc<std::sync::Mutex<Option<Arc<Heightmap>>>> =
            Arc::new(std::sync::Mutex::new(None));
        let recent_5m_close = Arc::clone(&recent_5m);
        let recent_5m_fine = Arc::clone(&recent_5m);

        let (hm5m_tx, hm5m_worker_rx) = mpsc::sync_channel::<(f64, f64)>(1);
        let (hm5m_worker_tx, hm5m_rx) = mpsc::channel::<TierData>();
        let close_idx = Arc::clone(&close_index);
        let lat_rad_5m = lat_rad;
        let base_hm_close = Arc::clone(hm);
        std::thread::spawn(move || {
            while let Ok((lat, lon)) = hm5m_worker_rx.recv() {
                let overlapping = tiles_overlapping_wgs84(&close_idx, lat, lon, close_radius_m);
                if overlapping.is_empty() {
                    continue;
                }
                let entry = &close_idx[overlapping[0]];
                let Ok((cx, cy)) = dem_io::crs::from_wgs84(lat, lon, &entry.crs_proj4) else {
                    continue;
                };
                let Ok(hm5m_raw) = extract_window(&entry.path, (cx, cy), close_radius_m, entry.ifd)
                else {
                    continue;
                };
                let mut hm5m_raw = cap_to_gpu_limit(hm5m_raw, cx, cy);
                dem_io::fill_nodata_from_base(&mut hm5m_raw, &base_hm_close);
                dem_io::clamp_nodata_to_sea(&mut hm5m_raw);
                let hm5m = Arc::new(hm5m_raw);
                if let Ok(mut g) = recent_5m_close.lock() {
                    *g = Some(Arc::clone(&hm5m));
                }
                let normals = terrain::compute_normals_vector_par(&hm5m);
                let (az, el) = sun_position(lat_rad_5m, INIT_SIM_DAY, INIT_SIM_HOUR);
                let shadow = terrain::compute_shadow_vector_par_with_azimuth(&hm5m, az, el, 200.0);
                let gpu_normals_rg16 =
                    render_gpu::pack_normals_rg16_bytes(&normals.nx, &normals.ny);
                // Use the *requested* centre, not the geometric window centre.
                // Near a tile edge the window is clipped, so its geometric centre drifts
                // away from the camera — triggering an infinite reload loop.
                if hm5m_worker_tx
                    .send(TierData {
                        hm: hm5m,
                        shadow,
                        centre_lat: lat,
                        centre_lon: lon,
                        gpu_normals_rg16,
                        gpu_normals_u32: vec![],
                        gpu_hm_f16: vec![],
                        gpu_hm_mips: vec![],
                        gpu_ao_u8: vec![],
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
        let mut effective_close_threshold = radii.close_drift_m;
        let overlapping_close =
            tiles_overlapping_wgs84(&close_index, cam_lat, cam_lon, close_radius_m);
        if let Some(&ci) = overlapping_close.first() {
            let entry = &close_index[ci];
            if let Ok((cx, cy)) = dem_io::crs::from_wgs84(cam_lat, cam_lon, &entry.crs_proj4)
                && let Ok(hm5m_init) =
                    extract_window(&entry.path, (cx, cy), close_radius_m, entry.ifd)
            {
                let mut hm5m_init = cap_to_gpu_limit(hm5m_init, cx, cy);
                dem_io::fill_nodata_from_base(&mut hm5m_init, hm);
                dem_io::clamp_nodata_to_sea(&mut hm5m_init);
                // When the GPU cap shrinks the window below close_radius_m (e.g. 1m tiles with no
                // overviews), keep the threshold at ≤ half the actual window half-extent so the
                // camera never exits the loaded window before a reload fires.
                let close_half_m = (hm5m_init.cols as f64 * hm5m_init.dx_meters)
                    .min(hm5m_init.rows as f64 * hm5m_init.dy_meters)
                    * 0.5;
                effective_close_threshold = radii.close_drift_m.min(close_half_m * 0.5);
                let (origin_x, origin_y, extent_x, extent_y, rot_rad) =
                    cross_crs_world_origin_and_extent(&hm5m_init, hm);
                let hm5m_init = Arc::new(hm5m_init);
                if let Ok(mut g) = recent_5m.lock() {
                    *g = Some(Arc::clone(&hm5m_init));
                }
                let normals5 = terrain::compute_normals_vector_par(&hm5m_init);
                let (az, el) = sun_position(lat_rad, INIT_SIM_DAY, INIT_SIM_HOUR);
                let shadow5 =
                    terrain::compute_shadow_vector_par_with_azimuth(&hm5m_init, az, el, 200.0);
                let normals5_rg16 = render_gpu::pack_normals_rg16_bytes(&normals5.nx, &normals5.ny);
                last_5m_lat = cam_lat;
                last_5m_lon = cam_lon;
                scene.upload_hm5m(
                    origin_x,
                    origin_y,
                    rot_rad,
                    extent_x,
                    extent_y,
                    &hm5m_init,
                    &normals5_rg16,
                    &shadow5,
                );
            }
        }

        // ── fine worker ────────────────────────────────────────────────────────────────
        // fine_radius_m == 0.0 is the runtime kill sentinel (set by the OOM
        // degradation path). None of the launcher presets currently set it to
        // zero — even Low loads a tiny 1 km fine window — but we keep the gate
        // so a future preset (or a hand-edited config) can opt out cleanly.
        let fine = if fine_index.is_empty() || fine_radius_m <= 0.0 {
            if fine_radius_m <= 0.0 {
                eprintln!("[tier] fine tier disabled (radius = 0)");
            }
            None
        } else {
            let (hm1m_tx, hm1m_worker_rx) = mpsc::sync_channel::<(f64, f64)>(1);
            let (hm1m_worker_tx, hm1m_rx) = mpsc::channel::<TierData>();
            let fine_idx = Arc::clone(&fine_index);
            let lat_rad_1m = lat_rad;
            let recent_5m_w = recent_5m_fine;
            std::thread::spawn(move || {
                while let Ok((lat, lon)) = hm1m_worker_rx.recv() {
                    let overlapping = tiles_overlapping_wgs84(&fine_idx, lat, lon, fine_radius_m);
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
                            extract_window(&e.path, (et, nt), fine_radius_m, e.ifd).ok()
                        })
                        .collect();
                    if windows.is_empty() {
                        continue;
                    }
                    let raw1m = if windows.len() == 1 {
                        windows.into_iter().next().unwrap()
                    } else {
                        stitch_windows(windows, e_tile, n_tile, fine_radius_m)
                    };
                    let mut raw1m = cap_to_gpu_limit(raw1m, e_tile, n_tile);
                    if let Ok(g) = recent_5m_w.lock()
                        && let Some(ref close_hm) = *g
                    {
                        dem_io::fill_nodata_from_base(&mut raw1m, close_hm);
                    }
                    dem_io::clamp_nodata_to_sea(&mut raw1m);
                    let hm1m = Arc::new(raw1m);
                    let normals = terrain::compute_normals_vector_par(&hm1m);
                    let (az, el) = sun_position(lat_rad_1m, INIT_SIM_DAY, INIT_SIM_HOUR);
                    let shadow =
                        terrain::compute_shadow_vector_par_with_azimuth(&hm1m, az, el, 200.0);
                    let gpu_normals_rg16 =
                        render_gpu::pack_normals_rg16_bytes(&normals.nx, &normals.ny);
                    if hm1m_worker_tx
                        .send(TierData {
                            hm: hm1m,
                            shadow,
                            centre_lat: lat,
                            centre_lon: lon,
                            gpu_normals_rg16,
                            gpu_normals_u32: vec![],
                            gpu_hm_f16: vec![],
                            gpu_hm_mips: vec![],
                            gpu_ao_u8: vec![],
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
                radii.fine_drift_m / M_PER_DEG,
            ))
        };

        // Base drift threshold: cap to half the actual window half-extent so that the camera
        // always stays inside the loaded window between reloads.  For large-overview tiles
        // the window >> base_radius_m and the constant wins; for GPU-capped tiles
        // (e.g. 1m NZ LiDAR, 8192 px = 8 km) the derived value is much smaller.
        let base_half_m = (hm.cols as f64 * hm.dx_meters).min(hm.rows as f64 * hm.dy_meters) * 0.5;
        let effective_base_threshold = radii.base_drift_m.min(base_half_m * 0.5);
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

/// Like `cross_crs_world_origin` but also returns `(extent_x, extent_y)` of `hm` in the base
/// world frame. When the two tiers share the same CRS this falls back to `cols*dx / rows*dy`.
/// When they differ, both the TR corner (for extent_x) and BL corner (for extent_y) of `hm`
/// are projected through WGS84 so the extents are consistent with the cos-scaled world metres
/// used for origin_x/y.
pub(super) fn cross_crs_world_origin_and_extent(
    hm: &Heightmap,
    base_hm: &Heightmap,
) -> (f32, f32, f32, f32, f32) {
    let (ox, oy) = cross_crs_world_origin(hm, base_hm);

    if hm.crs_proj4 == base_hm.crs_proj4 {
        return (
            ox,
            oy,
            (hm.cols as f64 * hm.dx_meters) as f32,
            (hm.rows as f64 * hm.dy_meters) as f32,
            0.0,
        );
    }

    // TR and BL corners of hm in its native CRS.
    let (tr_crs_x, bl_crs_y) = if dem_io::crs::is_geographic(&hm.crs_proj4) {
        (
            hm.crs_origin_x + hm.cols as f64 * hm.dx_deg,
            hm.crs_origin_y - hm.rows as f64 * hm.dy_deg.abs(),
        )
    } else {
        (
            hm.crs_origin_x + hm.cols as f64 * hm.dx_meters,
            hm.crs_origin_y - hm.rows as f64 * hm.dy_meters,
        )
    };

    let fallback = (
        ox,
        oy,
        hm.cols as f32 * hm.dx_meters as f32,
        hm.rows as f32 * hm.dy_meters as f32,
        0.0,
    );

    if dem_io::crs::is_geographic(&base_hm.crs_proj4) {
        let dx_m = base_hm.dx_deg * M_PER_DEG * base_hm.crs_origin_y.to_radians().cos();
        let dy_m = base_hm.dy_deg.abs() * M_PER_DEG;
        let Ok((tr_lat, tr_lon)) = dem_io::crs::to_wgs84(tr_crs_x, hm.crs_origin_y, &hm.crs_proj4)
        else {
            return fallback;
        };
        let Ok((bl_lat, bl_lon)) = dem_io::crs::to_wgs84(hm.crs_origin_x, bl_crs_y, &hm.crs_proj4)
        else {
            return fallback;
        };
        let tr_wx = ((tr_lon - base_hm.crs_origin_x) / base_hm.dx_deg * dx_m) as f32;
        let tr_wy = ((base_hm.crs_origin_y - tr_lat) / base_hm.dy_deg.abs() * dy_m) as f32;
        let bl_wx = ((bl_lon - base_hm.crs_origin_x) / base_hm.dx_deg * dx_m) as f32;
        let bl_wy = ((base_hm.crs_origin_y - bl_lat) / base_hm.dy_deg.abs() * dy_m) as f32;
        let ex = tr_wx - ox;
        let rot_rad = (tr_wy - oy).atan2(ex);

        let true_extent_x = ex.hypot(tr_wy - oy);
        let true_extent_y = (bl_wy - oy).hypot(bl_wx - ox);

        (ox, oy, true_extent_x, true_extent_y, rot_rad)
    } else {
        let Ok((tr_lat, tr_lon)) = dem_io::crs::to_wgs84(tr_crs_x, hm.crs_origin_y, &hm.crs_proj4)
        else {
            return fallback;
        };
        let Ok((tr_e, tr_n)) = dem_io::crs::from_wgs84(tr_lat, tr_lon, &base_hm.crs_proj4) else {
            return fallback;
        };
        let Ok((bl_lat, bl_lon)) = dem_io::crs::to_wgs84(hm.crs_origin_x, bl_crs_y, &hm.crs_proj4)
        else {
            return fallback;
        };
        let Ok((bl_e, bl_n)) = dem_io::crs::from_wgs84(bl_lat, bl_lon, &base_hm.crs_proj4) else {
            return fallback;
        };

        let tr_wx = (tr_e - base_hm.crs_origin_x) as f32;
        let tr_wy = (base_hm.crs_origin_y - tr_n) as f32;
        let bl_wx = (bl_e - base_hm.crs_origin_x) as f32;
        let bl_wy = (base_hm.crs_origin_y - bl_n) as f32;

        let ex = tr_wx - ox;
        let rot_rad = (tr_wy - oy).atan2(ex);

        let true_extent_x = ex.hypot(tr_wy - oy);
        let true_extent_y = (bl_wy - oy).hypot(bl_wx - ox);

        (ox, oy, true_extent_x, true_extent_y, rot_rad)
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
        // dx_meters is unreliable for geographic tiles; derive m/px from dx_deg.
        let dx_m = base_hm.dx_deg * M_PER_DEG * base_hm.crs_origin_y.to_radians().cos();
        let dy_m = base_hm.dy_deg.abs() * M_PER_DEG;
        let px = (lon - base_hm.crs_origin_x) / base_hm.dx_deg;
        let py = (base_hm.crs_origin_y - lat) / base_hm.dy_deg.abs();
        ((px * dx_m) as f32, (py * dy_m) as f32)
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── tier_radii ─────────────────────────────────────────────────────────

    #[test]
    fn tier_radii_are_internally_ordered() {
        for class in [VramClass::Low, VramClass::Mid, VramClass::High] {
            let r = tier_radii(class);
            assert!(
                r.base_radius_m > r.close_radius_m && r.close_radius_m > r.fine_radius_m,
                "{class:?}: radii must shrink base→close→fine"
            );
            // A drift threshold ≥ its radius would let the camera leave the
            // window before a reload ever fires.
            assert!(r.base_drift_m < r.base_radius_m, "{class:?} base drift");
            assert!(r.close_drift_m < r.close_radius_m, "{class:?} close drift");
            assert!(r.fine_drift_m < r.fine_radius_m, "{class:?} fine drift");
        }
    }

    #[test]
    fn higher_budget_never_shrinks_a_radius() {
        let lo = tier_radii(VramClass::Low);
        let mid = tier_radii(VramClass::Mid);
        let hi = tier_radii(VramClass::High);
        assert!(hi.base_radius_m >= mid.base_radius_m && mid.base_radius_m >= lo.base_radius_m);
        assert!(hi.close_radius_m >= mid.close_radius_m && mid.close_radius_m >= lo.close_radius_m);
        assert!(hi.fine_radius_m >= mid.fine_radius_m && mid.fine_radius_m >= lo.fine_radius_m);
    }

    // ── select_ifd ───────────────────────────────────────────────────────────

    #[test]
    fn select_ifd_picks_finest_level_meeting_scale_and_size() {
        let scales = [5.0, 25.0, 125.0];
        // base tier: want ≥ 30 m/px, 70 km radius, 8192 px cap.
        // 5 m and 25 m fail the scale floor; 125 m passes and its window
        // (70000·2/125 = 1120 px) fits the cap → level 2.
        assert_eq!(select_ifd(&scales, 30.0, 70_000.0, 8192), 2);
        // close tier: ≥ 4 m/px, 5 km radius → finest level 0 (5 m) qualifies.
        assert_eq!(select_ifd(&scales, 4.0, 5_000.0, 8192), 0);
    }

    #[test]
    fn select_ifd_falls_through_to_coarsest_when_nothing_fits() {
        // Single 5 m level, huge radius → 100000·2/5 = 40000 px blows the cap;
        // no level satisfies it, so the function returns the coarsest (len-1 = 0).
        assert_eq!(select_ifd(&[5.0], 4.0, 100_000.0, 8192), 0);
        let scales = [5.0, 25.0];
        assert_eq!(select_ifd(&scales, 4.0, 1_000_000.0, 8192), 1);
    }

    // ── cap_to_gpu_limit ─────────────────────────────────────────────────────

    fn proj_hm(rows: usize, cols: usize) -> Heightmap {
        Heightmap {
            data: vec![0.0; rows * cols],
            rows,
            cols,
            nodata: -9999.0,
            origin_lat: 0.0,
            origin_lon: 0.0,
            dx_deg: 0.0, // projected → cap uses dx_meters
            dy_deg: 0.0,
            dx_meters: 1.0,
            dy_meters: 1.0,
            crs_origin_x: 0.0,
            crs_origin_y: 0.0,
            crs_epsg: 32633,
            crs_proj4: String::new(),
        }
    }

    #[test]
    fn cap_is_noop_when_within_limit() {
        let hm = proj_hm(100, 100);
        let out = cap_to_gpu_limit(hm, 50.0, -50.0);
        assert_eq!((out.rows, out.cols), (100, 100));
    }

    #[test]
    fn cap_crops_oversized_axis_around_camera() {
        // 8200 px wide (over the 8192 limit), 4 px tall (under it).
        let hm = proj_hm(4, 8200);
        // Camera at easting 4100 → centred crop. col_start clamps to
        // min(4100-4096, 8200-8192) = min(4, 8) = 4 → origin shifts east by 4 m.
        let out = cap_to_gpu_limit(hm, 4100.0, 0.0);
        assert_eq!(out.cols, GPU_SAFE_PX);
        assert_eq!(out.rows, 4); // short axis untouched
        assert_eq!(out.crs_origin_x, 4.0);
    }

    // ── StreamingTier drift bookkeeping ──────────────────────────────────────

    fn streaming_tier(init_cx: f64, init_cy: f64, drift_m: f64) -> StreamingTier {
        // Worker-side endpoints are dropped immediately; these tests never send.
        let (tx, _worker_rx) = mpsc::sync_channel::<(f64, f64)>(1);
        let (_worker_tx, rx) = mpsc::channel::<TierData>();
        StreamingTier::new(tx, rx, init_cx, init_cy, drift_m)
    }

    #[test]
    fn needs_reload_fires_past_threshold_on_either_axis() {
        let t = streaming_tier(1000.0, 1000.0, 100.0);
        assert!(
            !t.needs_reload(1050.0, 1050.0),
            "within threshold on both axes"
        );
        assert!(t.needs_reload(1150.0, 1000.0), "x drift exceeds threshold");
        assert!(t.needs_reload(1000.0, 1150.0), "y drift exceeds threshold");
    }

    #[test]
    fn invalidate_forces_next_reload() {
        let mut t = streaming_tier(1000.0, 1000.0, 100.0);
        assert!(!t.needs_reload(1000.0, 1000.0));
        t.invalidate();
        // last_cx/cy are now 0, so any realistic CRS coordinate trips the check.
        assert!(t.needs_reload(1000.0, 1000.0));
    }

    #[test]
    fn update_threshold_changes_drift_sensitivity() {
        let mut t = streaming_tier(1000.0, 1000.0, 100.0);
        assert!(t.needs_reload(1150.0, 1000.0));
        t.update_threshold(2_000.0);
        assert!(
            !t.needs_reload(1150.0, 1000.0),
            "150 m drift is now within 2 km"
        );
    }

    #[test]
    fn try_recv_updates_centre_and_clears_computing() {
        let (tx, _worker_rx) = mpsc::sync_channel::<(f64, f64)>(1);
        let (worker_tx, rx) = mpsc::channel::<TierData>();
        let mut t = StreamingTier::new(tx, rx, 0.0, 0.0, 100.0);
        t.computing = true;

        let hm = Arc::new(proj_hm(2, 2));
        worker_tx
            .send(TierData {
                hm,
                shadow: ShadowMask {
                    data: vec![1.0; 4],
                    rows: 2,
                    cols: 2,
                },
                centre_lat: 47.5,
                centre_lon: 11.5,
                gpu_normals_rg16: Vec::new(),
                gpu_normals_u32: Vec::new(),
                gpu_hm_f16: Vec::new(),
                gpu_hm_mips: Vec::new(),
                gpu_ao_u8: Vec::new(),
            })
            .expect("send");

        let got = t.try_recv().expect("a bundle is queued");
        assert_eq!((got.centre_lat, got.centre_lon), (47.5, 11.5));
        assert!(!t.computing, "computing cleared once a bundle arrives");
        // Centre is now (47.5, 11.5); a nearby point no longer needs a reload.
        assert!(!t.needs_reload(47.51, 11.51));
    }

    #[test]
    fn try_recv_returns_none_when_idle() {
        let mut t = streaming_tier(0.0, 0.0, 100.0);
        assert!(t.try_recv().is_none());
    }
}
