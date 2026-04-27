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
/// Replaces the nine flat channel/drift fields that were previously in BevBaseState.
/// Adding a 1 m tier = add `fine: StreamingTier` here.
pub(super) struct BevBaseState {
    pub(super) base: StreamingTier,  // wide window, low resolution (IFD-2/1)
    pub(super) close: StreamingTier, // close window, 5 m/px (IFD-0)
}
