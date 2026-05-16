pub mod crs;
mod geotiff;
mod grid;
mod heightmap;
mod overview;

pub use crs::get_tile_epsg;
pub use geotiff::{
    extract_window, geotiff_pixel_scale, ifd_scales, parse_geotiff_auto, tile_centre_crs,
};
pub use grid::{assemble_grid, crop, load_grid, stitch_windows};
pub use heightmap::{Heightmap, parse_bil};
pub use overview::{BASE_OVERVIEW_TARGET_M, CLOSE_OVERVIEW_TARGET_M, ensure_overview_cache};

pub(crate) type DemError = Box<dyn std::error::Error>;
