pub mod crs;
mod geotiff;
mod grid;
mod heightmap;

pub use crs::get_tile_epsg;
pub use geotiff::{
    extract_window, geotiff_pixel_scale, ifd_scales, parse_geotiff_auto, tile_centre_crs,
};
pub use grid::{assemble_grid, crop, load_grid, stitch_windows};
pub use heightmap::{Heightmap, parse_bil};

pub(crate) type DemError = Box<dyn std::error::Error>;
