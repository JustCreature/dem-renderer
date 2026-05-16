pub(super) const WINDOW_W: u32 = 1600;
pub(super) const WINDOW_H: u32 = 533;
pub(super) const DEFAULT_CAM_LAT: f64 = 47.076211;
pub(super) const DEFAULT_CAM_LON: f64 = 11.687592;

// pub(super) const DEFAULT_TILE_5M_PATH: &str = "n47_e011_1arc_v3_bil/n47_e011_1arc_v3.bil";
// pub(super) const DEFAULT_TILE_5M_PATH: &str =
//     "tiles/Copernicus_DSM_COG_10_N47_00_E011_00_DEM/Copernicus_DSM_COG_10_N47_00_E011_00_DEM.tif";
// pub(super) const DEFAULT_TILE_5M_PATH: &str = "tiles/big_size/hintertux_5m.tif";
// pub(super) const DEFAULT_TILE_5M_PATH: &str = "tiles/big_size/hintertux_18km_5m.tif";
// pub(super) const DEFAULT_TILE_5M_PATH: &str = "tiles/big_size/hintertux_3km_1m.tif";
// pub(super) const DEFAULT_TILE_5M_PATH: &str = "tiles/big_size/hintertux_8km_1m.tif";
// pub(super) const DEFAULT_TILE_5M_PATH: &str = "tiles/big_size/salz_east_to_tux_base_8km_1m.tif";
pub(super) const DEFAULT_TILE_5M_PATH: &str = "tiles/big_size/DGM_R5.tif";

pub(super) const TILES_BIG_PATH: &str = "tiles/big_size/";

/// Maximum texture dimension accepted by wgpu without error.
/// Applied before every GPU upload so tiles with no overviews (e.g. 1m NZ LiDAR, 24000px wide)
/// never exceed the hardware texture dimension limit.
pub(super) const GPU_SAFE_PX: usize = 8192;
