mod geotiff;
mod grid;
mod heightmap;

pub use geotiff::{
    count_available_ifds, detect_projected_crs, extract_window, geotiff_pixel_scale, parse_geotiff,
    parse_geotiff_epsg_3035, parse_geotiff_epsg_31287,
};
pub use grid::{assemble_grid, crop, load_grid, stitch_windows};
pub use heightmap::{Heightmap, parse_bil};

pub(crate) type DemError = Box<dyn std::error::Error>;
