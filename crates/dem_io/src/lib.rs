mod alignment;
mod geotiff;
mod grid;
mod heightmap;
pub mod projection;

pub use alignment::estimate_alignment;
pub use geotiff::{
    count_available_ifds, extract_window, geotiff_pixel_scale, parse_geotiff,
    parse_geotiff_projected,
};
pub use grid::{assemble_grid, crop, load_grid, stitch_windows};
pub use heightmap::{Heightmap, parse_bil};
pub use projection::{Projection, Wgs84Identity, read_projection};

pub(crate) type DemError = Box<dyn std::error::Error>;
