use dem_io::Heightmap;
use dem_io::crs;

use crate::consts::M_PER_DEG;

/// Convert WGS84 lat/lon to tile-local metres (cam_pos.x, cam_pos.y).
/// Returns None if the position falls outside the tile bounds.
pub(super) fn latlon_to_tile_metres(lat: f64, lon: f64, hm: &Heightmap) -> Option<(f32, f32)> {
    let (x, y, max_x, max_y) = if crs::is_geographic(&hm.crs_proj4) {
        // dx_meters is unreliable for geographic tiles (may be deg/px or m/px depending
        // on which loader was used). Derive m/px consistently from dx_deg.
        let dx_m = hm.dx_deg * M_PER_DEG * hm.crs_origin_y.to_radians().cos();
        let dy_m = hm.dy_deg.abs() * M_PER_DEG;
        let x = (lon - hm.crs_origin_x) / hm.dx_deg * dx_m;
        let y = (hm.crs_origin_y - lat) / hm.dy_deg.abs() * dy_m;
        (x, y, hm.cols as f64 * dx_m, hm.rows as f64 * dy_m)
    } else {
        let (e, n) = crs::from_wgs84(lat, lon, &hm.crs_proj4).ok()?;
        let x = e - hm.crs_origin_x;
        let y = hm.crs_origin_y - n;
        (
            x,
            y,
            hm.cols as f64 * hm.dx_meters,
            hm.rows as f64 * hm.dy_meters,
        )
    };

    if x >= 0.0 && x <= max_x && y >= 0.0 && y <= max_y {
        Some((x as f32, y as f32))
    } else {
        None
    }
}

/// Geographic solar position (Spencer 1971 declination approximation).
/// Returns (azimuth_rad, elevation_rad) where azimuth is measured clockwise from North.
pub(super) fn sun_position(lat_rad: f32, day: i32, hour: f32) -> (f32, f32) {
    use std::f32::consts::TAU;
    // Solar declination
    let decl =
        23.45_f32.to_radians() * ((360.0_f32 / 365.0 * (day as f32 + 284.0)).to_radians()).sin();
    // Hour angle: 0 at solar noon, negative = morning
    let h = (15.0_f32 * (hour - 12.0)).to_radians();
    // Elevation
    let sin_el = lat_rad.sin() * decl.sin() + lat_rad.cos() * decl.cos() * h.cos();
    let elevation = sin_el.clamp(-1.0, 1.0).asin();
    // Azimuth from North, clockwise
    let cos_el = elevation.cos();
    let azimuth = if cos_el < 1e-6 {
        0.0
    } else {
        let cos_az = (decl.sin() - sin_el * lat_rad.sin()) / (cos_el * lat_rad.cos());
        let az = cos_az.clamp(-1.0, 1.0).acos();
        if h > 0.0 { TAU - az } else { az }
    };
    (azimuth, elevation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consts::M_PER_DEG;
    use std::f32::consts::PI;

    const LAT_47: f32 = 0.821_405; // 47.076211° in radians (DEFAULT_CAM_LAT)
    const SUMMER_SOLSTICE: i32 = 172; // ~21 June
    const WINTER_SOLSTICE: i32 = 355; // ~21 December

    // ── sun_position ─────────────────────────────────────────────────────────

    #[test]
    fn elevation_peaks_at_solar_noon() {
        let (_, el_noon) = sun_position(LAT_47, SUMMER_SOLSTICE, 12.0);
        let (_, el_morning) = sun_position(LAT_47, SUMMER_SOLSTICE, 9.0);
        let (_, el_afternoon) = sun_position(LAT_47, SUMMER_SOLSTICE, 15.0);
        assert!(el_noon > el_morning, "noon {el_noon} should top morning {el_morning}");
        assert!(el_noon > el_afternoon, "noon {el_noon} should top afternoon {el_afternoon}");
    }

    #[test]
    fn azimuth_straddles_south_across_noon() {
        // Northern mid-latitude: sun is due south (az = π) at solar noon, in the
        // eastern half (az < π) in the morning, western half (az > π) afternoon.
        // The afternoon branch is the `TAU - az` path.
        let (az_morning, _) = sun_position(LAT_47, SUMMER_SOLSTICE, 9.0);
        let (az_afternoon, _) = sun_position(LAT_47, SUMMER_SOLSTICE, 15.0);
        assert!(az_morning < PI, "morning azimuth {az_morning} should be east of south");
        assert!(az_afternoon > PI, "afternoon azimuth {az_afternoon} should be west of south");
        assert!(az_morning < az_afternoon);
    }

    #[test]
    fn summer_sun_higher_than_winter() {
        let (_, el_summer) = sun_position(LAT_47, SUMMER_SOLSTICE, 12.0);
        let (_, el_winter) = sun_position(LAT_47, WINTER_SOLSTICE, 12.0);
        assert!(
            el_summer > el_winter,
            "summer noon {el_summer} should exceed winter noon {el_winter}"
        );
    }

    // ── latlon_to_tile_metres ────────────────────────────────────────────────

    /// Geographic heightmap fixture. `dx_meters`/`dy_meters` are set to bogus
    /// values on purpose — the geographic branch must derive m/px from `dx_deg`.
    fn geo_hm() -> Heightmap {
        Heightmap {
            data: vec![0.0; 4],
            rows: 1000,
            cols: 1000,
            nodata: -9999.0,
            origin_lat: 47.0,
            origin_lon: 11.0,
            dx_deg: 0.001,
            dy_deg: -0.001,
            dx_meters: 999_999.0, // deliberately wrong; must be ignored
            dy_meters: 999_999.0,
            crs_origin_x: 11.0,
            crs_origin_y: 47.0,
            crs_epsg: 4326,
            crs_proj4: "+proj=longlat +datum=WGS84 +no_defs".to_string(),
        }
    }

    #[test]
    fn geographic_point_maps_using_dx_deg() {
        let hm = geo_hm();
        // 0.001° east, 0.001° south of the NW origin.
        let (x, y) = latlon_to_tile_metres(46.999, 11.001, &hm).expect("in bounds");
        // x = Δlon° · M_PER_DEG · cos(lat); y = Δlat° · M_PER_DEG.
        let expected_x = 0.001 * M_PER_DEG as f32 * 47.0_f32.to_radians().cos();
        let expected_y = 0.001 * M_PER_DEG as f32;
        assert!((x - expected_x).abs() < 0.5, "x = {x}, expected ≈ {expected_x}");
        assert!((y - expected_y).abs() < 0.5, "y = {y}, expected ≈ {expected_y}");
    }

    #[test]
    fn out_of_bounds_returns_none() {
        let hm = geo_hm();
        // West of the origin → negative x.
        assert!(latlon_to_tile_metres(47.0, 10.9, &hm).is_none());
        // North of the origin → negative y.
        assert!(latlon_to_tile_metres(47.5, 11.0, &hm).is_none());
        // Past the east edge: the tile spans exactly 1.0° (1000 × 0.001°), so
        // lon 12.0 is the edge — 12.1 is beyond it.
        assert!(latlon_to_tile_metres(46.5, 12.1, &hm).is_none());
    }
}
