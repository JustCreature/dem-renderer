mod aligned;
mod geotiff;
mod grid;
mod heightmap;
mod tiled;

pub use geotiff::{
    extract_window, geotiff_pixel_scale, parse_geotiff, parse_geotiff_epsg_3035,
    parse_geotiff_epsg_31287,
};
pub use grid::{assemble_grid, crop, load_grid, stitch_windows};
pub use heightmap::{Heightmap, parse_bil};
pub use tiled::TiledHeightmap;

pub(crate) type DemError = Box<dyn std::error::Error>;
