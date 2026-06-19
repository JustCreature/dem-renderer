//! Platform-agnostic terrain viewer core.
//!
//! Extracted from the `dem_renderer` binary's `src/viewer/` so the camera,
//! tier-streaming, render-orchestration and HUD logic can be reused by a wasm
//! shell (and any future non-winit front-end). Platform-touching seams live
//! behind the [`platform::TileSource`] / [`platform::Spawner`] traits; native
//! implementations are provided under `#[cfg(not(target_arch = "wasm32"))]`.

mod consts;

pub mod app;
pub mod camera;
pub mod geo;
pub mod hud;
pub mod platform;
pub mod scene_build;
pub mod tiers;
pub mod tile_index;

#[cfg(not(target_arch = "wasm32"))]
pub mod platform_native;

pub use app::{InitialView, PreparedScene, TierSetup, ViewerCore, ViewerSettings};
pub use hud::HudRenderer;
pub use platform::{Spawner, TileSource};
pub use tiers::BevBaseState;

pub use camera::FlyCamera;
pub use geo::{latlon_to_tile_metres, sun_position};
pub use scene_build::{INIT_SIM_DAY, INIT_SIM_HOUR, compute_ao_cropped};
pub use tiers::{
    AO_DRIFT_THRESHOLD_M, AO_RADIUS_M, StreamingTier, TierData, TierRadii, cap_to_gpu_limit,
    cross_crs_world_origin, cross_crs_world_origin_and_extent, select_ifd, tier_radii,
};
pub use tile_index::{TileEntry, TileIndex, tiles_overlapping_wgs84};

// Re-export the VRAM class so shells can resolve tier radii without depending on
// render_gpu directly.
pub use render_gpu::VramClass;
