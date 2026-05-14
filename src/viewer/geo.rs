use dem_io::{Heightmap, Projection};

/// Convert WGS84 lat/lon to tile-local metres (cam_pos.x, cam_pos.y).
/// Returns None if the position falls outside the tile bounds.
pub(super) fn latlon_to_tile_metres(
    lat: f64,
    lon: f64,
    hm: &Heightmap,
    proj: &dyn Projection,
) -> Option<(f32, f32)> {
    let (x, y) = if hm.dx_deg != 0.0 {
        // Geographic CRS: crs_origin_x = top-left longitude, crs_origin_y = top-left latitude
        let x = (lon - hm.crs_origin_x) / hm.dx_deg * hm.dx_meters;
        let y = (hm.crs_origin_y - lat) / hm.dy_deg.abs() * hm.dy_meters;
        (x, y)
    } else {
        let (easting, northing) = proj.forward(lat, lon);
        (easting - hm.crs_origin_x, hm.crs_origin_y - northing)
    };

    let max_x = hm.cols as f64 * hm.dx_meters;
    let max_y = hm.rows as f64 * hm.dy_meters;
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
    let decl =
        23.45_f32.to_radians() * ((360.0_f32 / 365.0 * (day as f32 + 284.0)).to_radians()).sin();
    let h = (15.0_f32 * (hour - 12.0)).to_radians();
    let sin_el = lat_rad.sin() * decl.sin() + lat_rad.cos() * decl.cos() * h.cos();
    let elevation = sin_el.clamp(-1.0, 1.0).asin();
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
