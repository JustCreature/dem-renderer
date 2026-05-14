use std::path::Path;
use std::sync::{Arc, mpsc};

use dem_io::{
    Heightmap, Projection, estimate_alignment, extract_window, geotiff_pixel_scale, load_grid,
    parse_geotiff, read_projection, stitch_windows,
};
use render_gpu::GpuScene;
use terrain::{NormalMap, ShadowMask};

use super::geo::sun_position;
use super::scene_init::{INIT_SIM_DAY, INIT_SIM_HOUR, compute_ao_cropped};

// BEV COG tier geometry.
// IFD-0 = 5 m/px  (full resolution, always present)
// IFD-1 ≈ 10 m/px (first overview)
// IFD-2 ≈ 20 m/px (second overview — preferred for the base window)
pub(super) const BEV_BASE_IFD: usize = 3;
pub(super) const BEV_BASE_RADIUS_M: f64 = 90_000.0;
pub(super) const BEV_BASE_DRIFT_THRESHOLD_M: f64 = 30_000.0;
pub(super) const BEV_5M_RADIUS_M: f64 = 20_000.0;
pub(super) const BEV_5M_DRIFT_THRESHOLD_M: f64 = 3_000.0;
pub(super) const BEV_1M_RADIUS_M: f64 = 3_500.0;
pub(super) const BEV_1M_DRIFT_THRESHOLD_M: f64 = 1_000.0;

pub(super) const AO_RADIUS_M: f64 = 20_000.0;
pub(super) const AO_DRIFT_THRESHOLD_M: f64 = 5_000.0;

/// A single tile found in a 1m directory, with coordinates parsed from its filename.
struct TileEntry {
    path: std::path::PathBuf,
    tile_n: f64,      // northing of SW corner in tile's CRS (from filename)
    tile_e: f64,      // easting of SW corner in tile's CRS (from filename)
    tile_size_m: f64, // parsed from RES{size}m in filename
}

/// Scan `dir` recursively for `CRS{code}RES{size}mN{n}E{e}*.tif` files.
/// The CRS code from the filename is not used in math — the projection is read from the
/// first tile file instead.
fn find_projected_tiles(dir: &Path) -> Vec<TileEntry> {
    let mut found = Vec::new();
    let Ok(walker) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in walker.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(find_projected_tiles(&path));
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".tif") {
            continue;
        }
        // Match `CRS{code}RES{size}m` prefix — strip everything up to and including the `m`
        let Some(crs_pos) = name.find("CRS") else {
            continue;
        };
        let after_crs = &name[crs_pos + 3..]; // skip "CRS"
        let Some(res_pos) = after_crs.find("RES") else {
            continue;
        };
        let after_res = &after_crs[res_pos + 3..]; // skip "RES"
        // after_res starts with digits then 'm'
        let m_pos = after_res.find('m').unwrap_or(0);
        let Ok(tile_size_m): Result<f64, _> = after_res[..m_pos].parse() else {
            continue;
        };
        let coord_part = &after_res[m_pos + 1..]; // skip 'm'

        let Some(n_pos) = coord_part.find('N') else {
            continue;
        };
        let Some(e_pos) = coord_part.find('E') else {
            continue;
        };
        if n_pos >= e_pos {
            continue;
        }
        let Ok(tile_n): Result<f64, _> = coord_part[n_pos + 1..e_pos].parse() else {
            continue;
        };
        // E value ends at the next non-digit character (dot before .tif)
        let e_str = &coord_part[e_pos + 1..];
        let e_end = e_str
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(e_str.len());
        let Ok(tile_e): Result<f64, _> = e_str[..e_end].parse() else {
            continue;
        };

        found.push(TileEntry {
            path,
            tile_n,
            tile_e,
            tile_size_m,
        });
    }
    found
}

/// Common result sent by any BEV background streaming worker.
pub(super) struct TierData {
    pub(super) hm: Arc<Heightmap>,
    pub(super) normals: NormalMap,
    pub(super) shadow: ShadowMask,
    pub(super) ao: Vec<f32>,
    pub(super) centre_e: f64,
    pub(super) centre_n: f64,
    /// Projection used by `hm` — needed for cross-CRS origin conversion in mod.rs.
    pub(super) proj: Arc<dyn Projection>,
}

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

    pub(super) fn needs_reload(&self, e: f64, n: f64) -> bool {
        (e - self.last_cx).abs() > self.drift_threshold_m
            || (n - self.last_cy).abs() > self.drift_threshold_m
    }

    pub(super) fn try_trigger(&mut self, e: f64, n: f64) -> bool {
        if self.tx.try_send((e, n)).is_ok() {
            self.computing = true;
            true
        } else {
            false
        }
    }

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

    pub(super) fn invalidate(&mut self) {
        self.computing = false;
        self.last_cx = 0.0;
        self.last_cy = 0.0;
    }
}

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

pub(super) struct Glo30State {
    pub(super) centre_lat: i32,
    pub(super) centre_lon: i32,
    pub(super) tile_tx: mpsc::SyncSender<(i32, i32, f64, f64)>,
    pub(super) tile_rx: mpsc::Receiver<TileBundle>,
    pub(super) tile_loading: bool,
}

pub(super) struct BevBaseState {
    pub(super) base: StreamingTier,
    pub(super) close: StreamingTier,
    pub(super) fine: Option<StreamingTier>,
    /// Projection of the base tile — used for cross-CRS fine-tier origin conversion.
    pub(super) proj: Arc<dyn Projection>,
    /// Alignment correction derived from phase correlation at startup.
    /// `Some((dx,dy))` when phase-correlation succeeded; `None` when tiles don't overlap.
    pub(super) computed_align5m: Option<(f32, f32)>,
    /// Last close-tier heightmap — reference snapshot for auto-aligning the 1m fine tier.
    pub(super) last_close_hm: Option<Arc<Heightmap>>,
    /// Projection for `last_close_hm`.
    pub(super) last_close_proj: Arc<dyn Projection>,
}

impl BevBaseState {
    /// Spawn all three background worker threads and return the populated state.
    ///
    /// `proj` — the projection for `tile_path`'s CRS, read once by the caller.
    /// `single_file_fine` — when true, the fine tier is read from `tile_path` at IFD-0
    ///   instead of scanning `tiles_1m_dir`.
    pub(super) fn new(
        tile_path: &Path,
        proj: Arc<dyn Projection>,
        base_proj: Arc<dyn Projection>,
        single_file_fine: bool,
        lat_rad: f32,
        init_e: f64,
        init_n: f64,
        hm: &Arc<Heightmap>,
        tiles_1m_dir: Option<&Path>,
        align5m: (f32, f32, f32),
        _align1m: (f32, f32, f32),
        scene: &mut GpuScene,
    ) -> Self {
        let native_px_m = geotiff_pixel_scale(tile_path);
        let base_ifd = {
            let doublings = (BEV_BASE_RADIUS_M * 2.0 / 8000.0 / native_px_m)
                .log2()
                .ceil()
                .max(0.0) as usize;
            doublings
        };
        let (close_ifd, close_radius_m) = if native_px_m <= 1.5 {
            (3usize, 16_000.0_f64)
        } else {
            (0usize, BEV_5M_RADIUS_M)
        };

        // ── base drift worker ──────────────────────────────────────────────────────────
        let tile_path_base = tile_path.to_path_buf();
        let (base_tx, base_worker_rx) = mpsc::sync_channel::<(f64, f64)>(1);
        let (base_worker_tx, base_rx) = mpsc::channel::<TierData>();
        let lat_rad_w = lat_rad;
        let proj_base = Arc::clone(&proj);
        std::thread::spawn(move || {
            while let Ok((easting, northing)) = base_worker_rx.recv() {
                let hm_result = extract_window(
                    &tile_path_base,
                    (easting, northing),
                    BEV_BASE_RADIUS_M,
                    base_ifd,
                    &proj_base,
                )
                .or_else(|_| {
                    extract_window(
                        &tile_path_base,
                        (easting, northing),
                        BEV_BASE_RADIUS_M,
                        base_ifd.saturating_sub(1),
                        &proj_base,
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
                        proj: Arc::clone(&proj_base),
                    })
                    .is_err()
                {
                    break;
                }
            }
        });

        // ── close-tier drift worker ───────────────────────────────────────────────────
        let tile_path_5m = tile_path.to_path_buf();
        let (hm5m_tx, hm5m_worker_rx) = mpsc::sync_channel::<(f64, f64)>(1);
        let (hm5m_worker_tx, hm5m_rx) = mpsc::channel::<TierData>();
        let lat_rad_5m = lat_rad;
        let proj_close = Arc::clone(&proj);
        std::thread::spawn(move || {
            while let Ok((easting, northing)) = hm5m_worker_rx.recv() {
                let Ok(hm5m) = extract_window(
                    &tile_path_5m,
                    (easting, northing),
                    close_radius_m,
                    close_ifd,
                    &proj_close,
                ) else {
                    continue;
                };
                let hm5m = Arc::new(hm5m);
                let normals = terrain::compute_normals_vector_par(&hm5m);
                let (az, el) = sun_position(lat_rad_5m, INIT_SIM_DAY, INIT_SIM_HOUR);
                let shadow = terrain::compute_shadow_vector_par_with_azimuth(&hm5m, az, el, 200.0);
                if hm5m_worker_tx
                    .send(TierData {
                        hm: hm5m,
                        normals,
                        shadow,
                        ao: vec![],
                        centre_e: easting,
                        centre_n: northing,
                        proj: Arc::clone(&proj_close),
                    })
                    .is_err()
                {
                    break;
                }
            }
        });

        // ── blocking initial close-tier load ──────────────────────────────────────────
        let mut last_5m_cx = 0.0_f64;
        let mut last_5m_cy = 0.0_f64;
        let mut computed_align5m: Option<(f32, f32)> = None;
        let mut last_close_hm: Option<Arc<Heightmap>> = None;
        if let Ok(hm5m_init) = extract_window(
            tile_path,
            (init_e, init_n),
            close_radius_m,
            close_ifd,
            &proj,
        ) {
            let hm5m_init = Arc::new(hm5m_init);
            last_close_hm = Some(Arc::clone(&hm5m_init));
            let origin_x = (hm5m_init.crs_origin_x - hm.crs_origin_x) as f32;
            let origin_y = (hm.crs_origin_y - hm5m_init.crs_origin_y) as f32;
            let normals5 = terrain::compute_normals_vector_par(&hm5m_init);
            let (az, el) = sun_position(lat_rad, INIT_SIM_DAY, INIT_SIM_HOUR);
            let shadow5 =
                terrain::compute_shadow_vector_par_with_azimuth(&hm5m_init, az, el, 200.0);
            last_5m_cx = init_e;
            last_5m_cy = init_n;

            // Auto-compute alignment via phase correlation between base and close tier.
            let t_align = std::time::Instant::now();
            computed_align5m = estimate_alignment(hm, &*base_proj, &hm5m_init, &*proj);
            match computed_align5m {
                Some((dx, dy)) => println!(
                    "auto-align 5m: ({:.1}m, {:.1}m)  ({:.2?})",
                    dx, dy, t_align.elapsed()
                ),
                None => println!(
                    "auto-align 5m: no overlap or ambiguous, using saved ({:.2?})",
                    t_align.elapsed()
                ),
            }

            // Use auto-computed when available; fall back to caller-supplied only on failure.
            let (eff_dx, eff_dy) = computed_align5m.unwrap_or((align5m.0, align5m.1));

            println!(
                "close tier initial: {}×{} at {:.1}m/px (IFD-{})",
                hm5m_init.cols, hm5m_init.rows, hm5m_init.dx_meters, close_ifd
            );
            scene.upload_hm5m(
                origin_x,
                origin_y,
                eff_dx,
                eff_dy,
                align5m.2.to_radians(),
                &hm5m_init,
                &normals5,
                &shadow5,
            );
        } else {
            println!("warning: could not load initial close-tier window");
        }

        // ── 1m fine-tier worker ────────────────────────────────────────────────────────
        enum Fine1mSource {
            SingleFile {
                path: std::path::PathBuf,
                proj: Arc<dyn Projection>,
            },
            Directory {
                dir: std::path::PathBuf,
                base_proj: Arc<dyn Projection>,
            },
        }

        let fine_source: Option<Fine1mSource> = if single_file_fine {
            Some(Fine1mSource::SingleFile {
                path: tile_path.to_path_buf(),
                proj: Arc::clone(&proj),
            })
        } else {
            tiles_1m_dir.map(|dir| Fine1mSource::Directory {
                dir: dir.to_path_buf(),
                base_proj: Arc::clone(&proj),
            })
        };

        let fine = fine_source.map(|source| {
            let (hm1m_tx, hm1m_worker_rx) = mpsc::sync_channel::<(f64, f64)>(1);
            let (hm1m_worker_tx, hm1m_rx) = mpsc::channel::<TierData>();
            let lat_rad_1m = lat_rad;
            std::thread::spawn(move || {
                while let Ok((easting, northing)) = hm1m_worker_rx.recv() {
                    let (tile_paths, window_e, window_n, fine_proj) = match &source {
                        Fine1mSource::SingleFile { path, proj } => {
                            (vec![path.clone()], easting, northing, Arc::clone(proj))
                        }
                        Fine1mSource::Directory { dir, base_proj } => {
                            let candidates = find_projected_tiles(dir);
                            if candidates.is_empty() {
                                continue;
                            }
                            // Read projection from first tile file — all tiles share one CRS.
                            let tile_proj = match read_projection(&candidates[0].path) {
                                Ok(p) => p,
                                Err(e) => {
                                    eprintln!("cannot read CRS from 1m tile: {e}");
                                    continue;
                                }
                            };
                            // Convert camera position from base CRS → WGS84 → tile CRS.
                            let (lat, lon) = base_proj.inverse(easting, northing);
                            let (cam_e, cam_n) = tile_proj.forward(lat, lon);
                            let nearby: Vec<_> = candidates
                                .iter()
                                .filter(|t| {
                                    t.tile_e < cam_e + BEV_1M_RADIUS_M
                                        && t.tile_e + t.tile_size_m > cam_e - BEV_1M_RADIUS_M
                                        && t.tile_n < cam_n + BEV_1M_RADIUS_M
                                        && t.tile_n + t.tile_size_m > cam_n - BEV_1M_RADIUS_M
                                })
                                .map(|t| t.path.clone())
                                .collect();
                            (nearby, cam_e, cam_n, tile_proj)
                        }
                    };
                    if tile_paths.is_empty() {
                        continue;
                    }
                    let windows: Vec<_> = tile_paths
                        .iter()
                        .filter_map(|p| {
                            extract_window(p, (window_e, window_n), BEV_1M_RADIUS_M, 0, &fine_proj)
                                .ok()
                        })
                        .collect();
                    if windows.is_empty() {
                        continue;
                    }
                    let hm1m = Arc::new(if windows.len() == 1 {
                        windows.into_iter().next().unwrap()
                    } else {
                        stitch_windows(windows, window_e, window_n, BEV_1M_RADIUS_M)
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
                            proj: Arc::clone(&fine_proj),
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
            last_close_hm,
            last_close_proj: Arc::clone(&proj),
            proj,
            computed_align5m,
        }
    }
}

impl Glo30State {
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
