mod aligned;
mod geotiff;
mod heightmap;
mod tiled;

pub use geotiff::{geotiff_is_projected, parse_geotiff, parse_geotiff_epsg_31287};
pub use heightmap::{Heightmap, parse_bil};
pub use tiled::TiledHeightmap;

pub(crate) type DemError = Box<dyn std::error::Error>;
