//! Native (filesystem + `std::thread`) implementations of the platform traits.
//! Compiled only off-wasm; a wasm shell supplies its own adapters.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};

use dem_io::{Heightmap, extract_window};

use crate::platform::{Spawner, TileSource};
use crate::tile_index::{TileEntry, TileIndex};

/// Filesystem-backed [`TileSource`] wrapping the `dem_io` `&Path` readers.
pub struct NativeTileSource;

impl TileSource for NativeTileSource {
    fn read_window(
        &self,
        key: &Path,
        centre_crs: (f64, f64),
        radius_m: f64,
        ifd: usize,
    ) -> Result<Heightmap, String> {
        extract_window(key, centre_crs, radius_m, ifd).map_err(|e| e.to_string())
    }

    fn read_full(&self, key: &Path) -> Result<Heightmap, String> {
        dem_io::parse_geotiff_auto(key).map_err(|e| e.to_string())
    }

    fn tile_centre_crs(&self, key: &Path) -> Result<(f64, f64), String> {
        dem_io::tile_centre_crs(key).map_err(|e| e.to_string())
    }

    fn ifd_scales(&self, key: &Path) -> Result<Vec<f64>, String> {
        dem_io::ifd_scales(key).map_err(|e| e.to_string())
    }
}

/// `std::thread::spawn`-backed [`Spawner`].
pub struct ThreadSpawner;

impl Spawner for ThreadSpawner {
    fn spawn(&self, job: Box<dyn FnOnce() + Send + 'static>) {
        std::thread::spawn(job);
    }
}

/// Build a `TileIndex` from an explicit list of paths. Missing files or files
/// that fail CRS / bounds extraction are silently skipped (graceful
/// degradation). This is I/O (it opens each tile), so it lives in the native
/// adapter rather than the portable `tile_index` module.
pub fn build_tile_index(paths: &[PathBuf]) -> TileIndex {
    let mut index = Vec::new();
    for path in paths {
        if !path.exists() {
            continue;
        }
        let Ok(proj4) = dem_io::crs::tile_proj4(path) else {
            continue;
        };
        let Ok((lat_min, lat_max, lon_min, lon_max)) = dem_io::tile_bounds_wgs84(path) else {
            continue;
        };
        index.push(TileEntry {
            path: path.clone(),
            ifd: 0,
            crs_proj4: proj4,
            lat_min,
            lat_max,
            lon_min,
            lon_max,
        });
    }
    index
}
