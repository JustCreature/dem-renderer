use dem_io::Heightmap;

/// Lambert Conformal Conic forward projection for EPSG:31287 (MGI / Austria Lambert).
/// Input: WGS84 lat/lon in degrees. Output: (easting, northing) in metres.
pub(super) fn lcc_epsg31287(lat_deg: f64, lon_deg: f64) -> (f64, f64) {
    // Bessel 1841 ellipsoid
    let a = 6_377_397.155_f64;
    let f = 1.0 / 299.152_812_8_f64;
    let e2 = 2.0 * f - f * f;
    let e = e2.sqrt();

    let to_rad = std::f64::consts::PI / 180.0;
    let lat = lat_deg * to_rad;
    let lon = lon_deg * to_rad;

    // EPSG:31287 defining parameters
    let lat0 = 47.5 * to_rad; // latitude of false origin
    let lon0 = 13.333_333 * to_rad; // central meridian
    let lat1 = 49.0 * to_rad; // standard parallel 1
    let lat2 = 46.0 * to_rad; // standard parallel 2
    let fe = 400_000.0_f64; // false easting
    let fn_ = 400_000.0_f64; // false northing

    // Helper: m (Snyder eq 15-11)
    let m = |phi: f64| {
        let sin_phi = phi.sin();
        phi.cos() / (1.0 - e2 * sin_phi * sin_phi).sqrt()
    };
    // Helper: t (Snyder eq 15-9)
    let t = |phi: f64| {
        let sin_phi = phi.sin();
        let e_sin = e * sin_phi;
        ((1.0 - sin_phi) / (1.0 + sin_phi) * ((1.0 + e_sin) / (1.0 - e_sin)).powf(e)).sqrt()
    };

    let m1 = m(lat1);
    let m2 = m(lat2);
    let t0 = t(lat0);
    let t1 = t(lat1);
    let t2 = t(lat2);

    let n = (m1.ln() - m2.ln()) / (t1.ln() - t2.ln());
    let big_f = m1 / (n * t1.powf(n));
    let rho0 = a * big_f * t0.powf(n);
    let rho = a * big_f * t(lat).powf(n);
    let theta = n * (lon - lon0);

    let easting = fe + rho * theta.sin();
    let northing = fn_ + rho0 - rho * theta.cos();
    (easting, northing)
}

/// Spherical LAEA forward projection for EPSG:3035 (ETRS89 / LAEA Europe).
/// Input: WGS84 lat/lon degrees. Output: (easting, northing) metres.
pub(super) fn laea_epsg3035(lat_deg: f64, lon_deg: f64) -> (f64, f64) {
    let r = 6_371_000.0_f64;
    let lat = lat_deg.to_radians();
    let lon = lon_deg.to_radians();
    let lat0 = 52.0_f64.to_radians();
    let lon0 = 10.0_f64.to_radians();
    let fe = 4_321_000.0_f64;
    let fn_ = 3_210_000.0_f64;

    let k =
        (2.0 / (1.0 + lat0.sin() * lat.sin() + lat0.cos() * lat.cos() * (lon - lon0).cos())).sqrt();
    let easting = fe + r * k * lat.cos() * (lon - lon0).sin();
    let northing =
        fn_ + r * k * (lat0.cos() * lat.sin() - lat0.sin() * lat.cos() * (lon - lon0).cos());
    (easting, northing)
}

/// Spherical LAEA inverse for EPSG:3035. Returns (lat_deg, lon_deg). Accuracy ~100m.
pub(super) fn laea_epsg3035_inverse(easting: f64, northing: f64) -> (f64, f64) {
    let r = 6_371_000.0_f64;
    let lat0 = 52.0_f64.to_radians();
    let lon0 = 10.0_f64.to_radians();
    let fe = 4_321_000.0_f64;
    let fn_ = 3_210_000.0_f64;
    let x = easting - fe;
    let y = northing - fn_;
    let rho = (x * x + y * y).sqrt();
    if rho < 1e-10 {
        return (52.0, 10.0);
    }
    let c = 2.0 * (rho / (2.0 * r)).asin();
    let lat = (c.cos() * lat0.sin() + y * c.sin() * lat0.cos() / rho).asin();
    let lon = lon0 + (x * c.sin()).atan2(rho * lat0.cos() * c.cos() - y * lat0.sin() * c.sin());
    (lat.to_degrees(), lon.to_degrees())
}

/// LCC inverse for EPSG:31287 (MGI / Austria Lambert). Returns (lat_deg, lon_deg).
/// Iterates to convergence (typically 4–5 iterations).
pub(super) fn lcc_epsg31287_inverse(easting: f64, northing: f64) -> (f64, f64) {
    let a = 6_377_397.155_f64;
    let f = 1.0 / 299.152_812_8_f64;
    let e2 = 2.0 * f - f * f;
    let e = e2.sqrt();
    let to_rad = std::f64::consts::PI / 180.0;

    let lat0 = 47.5 * to_rad;
    let lon0 = 13.333_333 * to_rad;
    let lat1 = 49.0 * to_rad;
    let lat2 = 46.0 * to_rad;
    let fe = 400_000.0_f64;
    let fn_ = 400_000.0_f64;

    let m = |phi: f64| {
        let s = phi.sin();
        phi.cos() / (1.0 - e2 * s * s).sqrt()
    };
    let t = |phi: f64| {
        let s = phi.sin();
        let es = e * s;
        ((1.0 - s) / (1.0 + s) * ((1.0 + es) / (1.0 - es)).powf(e)).sqrt()
    };

    let m1 = m(lat1);
    let m2 = m(lat2);
    let t1 = t(lat1);
    let t2 = t(lat2);
    let t0 = t(lat0);
    let n = (m1.ln() - m2.ln()) / (t1.ln() - t2.ln());
    let big_f = m1 / (n * t1.powf(n));
    let rho0 = a * big_f * t0.powf(n);

    let x = easting - fe;
    let y = northing - fn_;
    let rho_inv = (x * x + (rho0 - y) * (rho0 - y)).sqrt() * n.signum();
    let theta_inv = (x).atan2(rho0 - y);
    let t_inv = (rho_inv / (a * big_f)).powf(1.0 / n);

    // Iterative latitude solution (Snyder p. 45)
    let mut phi = std::f64::consts::FRAC_PI_2 - 2.0 * t_inv.atan();
    for _ in 0..10 {
        let es = e * phi.sin();
        phi = std::f64::consts::FRAC_PI_2
            - 2.0 * (t_inv * ((1.0 - es) / (1.0 + es)).powf(e / 2.0)).atan();
    }
    let lon = theta_inv / n + lon0;
    (phi.to_degrees(), lon.to_degrees())
}

/// Convert WGS84 lat/lon to tile-local metres (cam_pos.x, cam_pos.y).
/// Returns None if the position falls outside the tile bounds.
pub(super) fn latlon_to_tile_metres(lat: f64, lon: f64, hm: &Heightmap) -> Option<(f32, f32)> {
    let (x, y) = match hm.crs_epsg {
        31287 => {
            let (easting, northing) = lcc_epsg31287(lat, lon);
            (easting - hm.crs_origin_x, hm.crs_origin_y - northing)
        }
        3035 => {
            let (easting, northing) = laea_epsg3035(lat, lon);
            (easting - hm.crs_origin_x, hm.crs_origin_y - northing)
        }
        _ => {
            // Geographic (EPSG:4326)
            let x = (lon - hm.crs_origin_x) / hm.dx_deg * hm.dx_meters;
            let y = (hm.crs_origin_y - lat) / hm.dy_deg.abs() * hm.dy_meters;
            (x, y)
        }
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
