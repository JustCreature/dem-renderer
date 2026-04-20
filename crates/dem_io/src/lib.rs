mod aligned;
mod geotiff;
mod heightmap;
mod tiled;

pub use geotiff::parse_geotiff;
pub use heightmap::{Heightmap, parse_bil};
pub use tiled::TiledHeightmap;

pub(crate) type DemError = Box<dyn std::error::Error>;
