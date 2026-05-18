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
