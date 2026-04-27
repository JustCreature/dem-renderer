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

/// Result sent by the BEV base-tier background worker when a new 60 km window
/// (IFD-2 ≈ 20 m/px, fallback IFD-1 ≈ 10 m/px) finishes loading.
pub(super) struct BevBaseBundle {
    pub(super) hm: Arc<Heightmap>,
    pub(super) normals: NormalMap,
    pub(super) shadow: ShadowMask,
    pub(super) ao: Vec<f32>,
    pub(super) cam_e: f64, // EPSG:31287 easting the window was centred on
    pub(super) cam_n: f64,
}

/// Result sent by the BEV close-tier background worker when a new 10 km window
/// at IFD-0 (5 m/px, full resolution) finishes loading.
pub(super) struct Hm5mBundle {
    pub(super) hm5m: Arc<Heightmap>,
    pub(super) normals: NormalMap,
    pub(super) shadow: ShadowMask,
}

/// Persistent state for BEV two-tier mode.
/// Owns the worker channels for both the wide base window and the 5 m close tier,
/// plus the last-known window centres used for drift detection.
pub(super) struct BevBaseState {
    pub(super) base_tx: mpsc::SyncSender<(f64, f64)>,
    pub(super) base_rx: mpsc::Receiver<BevBaseBundle>,
    pub(super) loading: bool,
    pub(super) last_cx: f64, // EPSG:31287 easting of last base window centre
    pub(super) last_cy: f64, // EPSG:31287 northing of last base window centre
    // 5m close tier
    pub(super) hm5m_tx: mpsc::SyncSender<(f64, f64)>,
    pub(super) hm5m_rx: mpsc::Receiver<Hm5mBundle>,
    pub(super) hm5m_computing: bool,
    pub(super) last_5m_cx: f64, // EPSG:31287 easting of last 5m window centre
    pub(super) last_5m_cy: f64,
}
