use std::path::PathBuf;

pub struct TileEntry {
    pub path: PathBuf,
    /// IFD level to use when calling extract_window (0 = finest).
    /// Set to 0 by build_tile_index; callers may override for coarser tiers.
    pub ifd: usize,
    pub crs_proj4: String,
    pub lat_min: f64,
    pub lat_max: f64,
    pub lon_min: f64,
    pub lon_max: f64,
}

pub type TileIndex = Vec<TileEntry>;

/// Build a TileIndex from an explicit list of paths.  Missing files or files that
/// fail CRS / bounds extraction are silently skipped (graceful degradation).
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

/// Return indices of TileIndex entries whose WGS84 bounds overlap a box of
/// `radius_m` metres around `(lat, lon)`.
pub fn tiles_overlapping_wgs84(index: &TileIndex, lat: f64, lon: f64, radius_m: f64) -> Vec<usize> {
    let dlat = radius_m / crate::consts::M_PER_DEG;
    let dlon = radius_m / (crate::consts::M_PER_DEG * lat.to_radians().cos());
    index
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            e.lat_max > lat - dlat
                && e.lat_min < lat + dlat
                && e.lon_max > lon - dlon
                && e.lon_min < lon + dlon
        })
        .map(|(i, _)| i)
        .collect()
}
