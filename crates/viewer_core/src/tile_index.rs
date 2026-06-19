use std::path::PathBuf;

pub struct TileEntry {
    /// Opaque key identifying the tile to the [`crate::platform::TileSource`].
    /// Native impl treats it as a filesystem path; a wasm impl may treat it as
    /// a lookup key into in-memory bytes.
    pub path: PathBuf,
    /// IFD level to use when reading a window (0 = finest).
    /// Set to 0 by `build_tile_index`; callers may override for coarser tiers.
    pub ifd: usize,
    pub crs_proj4: String,
    pub lat_min: f64,
    pub lat_max: f64,
    pub lon_min: f64,
    pub lon_max: f64,
}

pub type TileIndex = Vec<TileEntry>;

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

#[cfg(test)]
mod tests {
    use super::*;

    // `build_tile_index` reads real GeoTIFFs (CRS + bounds extraction) and lives
    // in `platform_native` (it is I/O) — not unit-tested here.
    // These tests cover the pure geometry of `tiles_overlapping_wgs84`.

    fn entry(lat_min: f64, lat_max: f64, lon_min: f64, lon_max: f64) -> TileEntry {
        TileEntry {
            path: PathBuf::from("dummy.tif"),
            ifd: 0,
            crs_proj4: String::new(),
            lat_min,
            lat_max,
            lon_min,
            lon_max,
        }
    }

    #[test]
    fn returns_overlapping_excludes_distant() {
        let index = vec![
            entry(46.9, 47.1, 10.9, 11.1), // straddles the query point
            entry(40.0, 41.0, 5.0, 6.0),   // far to the south-west
        ];
        let hits = tiles_overlapping_wgs84(&index, 47.0, 11.0, 5_000.0);
        assert_eq!(hits, vec![0]);
    }

    #[test]
    fn longitude_margin_widens_with_latitude() {
        // Same metre radius and the same 0.13° longitude gap from the query:
        // at 60°N the 1/cos(lat) widening makes the box overlap, but at the
        // equator the narrower longitude band does not reach it.
        let radius = 10_000.0;
        let high_lat = vec![entry(59.0, 61.0, 10.13, 10.14)];
        let equator = vec![entry(-1.0, 1.0, 10.13, 10.14)];

        assert_eq!(
            tiles_overlapping_wgs84(&high_lat, 60.0, 10.0, radius),
            vec![0],
            "0.13° gap is within the widened band at 60°N"
        );
        assert!(
            tiles_overlapping_wgs84(&equator, 0.0, 10.0, radius).is_empty(),
            "0.13° gap is outside the narrow band at the equator"
        );
    }

    #[test]
    fn just_inside_vs_just_outside_margin() {
        // dlat = radius / M_PER_DEG. At radius 5 km that's ≈ 0.0449°.
        let radius = 5_000.0;
        let dlat = radius / crate::consts::M_PER_DEG;
        let inside = vec![entry(47.0 + dlat * 0.5, 48.0, 10.99, 11.01)];
        let outside = vec![entry(47.0 + dlat * 2.0, 48.0, 10.99, 11.01)];
        assert_eq!(tiles_overlapping_wgs84(&inside, 47.0, 11.0, radius), vec![0]);
        assert!(tiles_overlapping_wgs84(&outside, 47.0, 11.0, radius).is_empty());
    }
}
