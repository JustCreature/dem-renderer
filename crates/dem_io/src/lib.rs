mod heightmap;
mod tiled;

pub use heightmap::{Heightmap, parse_bil};

pub(crate) type DemError = Box<dyn std::error::Error>;
