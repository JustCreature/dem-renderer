use std::path::Path;
use std::sync::{mpsc, Arc};

use dem_io::Heightmap;
use terrain::{NormalMap, ShadowMask};

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
pub(super) fn find_1m_tiles(dir: &Path, e3035: f64, n3035: f64, radius_m: f64) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let Ok(walker) = std::fs::read_dir(dir) else { return found };
    for entry in walker.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(find_1m_tiles(&path, e3035, n3035, radius_m));
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        if !name.starts_with("CRS3035RES50000m") || !name.ends_with(".tif") {
            continue;
        }
        let Some(rest) = name.strip_prefix("CRS3035RES50000m").and_then(|r| r.strip_suffix(".tif")) else { continue };
        let Some(n_pos) = rest.find('N') else { continue };
        let Some(e_pos) = rest.find('E') else { continue };
        if n_pos >= e_pos { continue; }
        let Ok(tile_n): Result<f64, _> = rest[n_pos + 1..e_pos].parse() else { continue };
        let Ok(tile_e): Result<f64, _> = rest[e_pos + 1..].parse() else { continue };
        // tile covers [tile_e, tile_e+BEV_TILE_SIZE_M) × [tile_n, tile_n+BEV_TILE_SIZE_M)
        if tile_e < e3035 + radius_m && tile_e + BEV_TILE_SIZE_M > e3035 - radius_m
            && tile_n < n3035 + radius_m && tile_n + BEV_TILE_SIZE_M > n3035 - radius_m
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
    pub(super) ao: Vec<f32>,   // empty Vec for tiers that do not compute AO
    pub(super) centre_e: f64,  // absolute CRS easting of the loaded window centre
    pub(super) centre_n: f64,  // absolute CRS northing of the loaded window centre
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
    pub(super) base: StreamingTier,          // wide window, low resolution (IFD-2/1)
    pub(super) close: StreamingTier,         // close window, 5 m/px (IFD-0)
    pub(super) fine: Option<StreamingTier>,  // fine window, 1 m/px (1m tile IFD-0); None if no 1m tiles available
}
