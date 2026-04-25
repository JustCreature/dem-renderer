use std::fs::File;
use std::path::Path;
use tiff::decoder::{Decoder, DecodingResult, Limits};
use tiff::tags::Tag;

use crate::heightmap::fill_nodata;
use crate::{DemError, Heightmap};

/// Returns ModelPixelScaleTag[0] from a GeoTIFF, or 0.0 on failure.
/// Geographic CRS: < 0.1 (degrees/pixel). Projected: >= 1.0 (metres/pixel).
pub fn geotiff_pixel_scale(path: &Path) -> f64 {
    let Ok(file) = File::open(path) else {
        return 0.0;
    };
    let Ok(mut decoder) = Decoder::new(std::io::BufReader::new(file)) else {
        return 0.0;
    };
    let Ok(scale) = decoder
        .get_tag(Tag::Unknown(33550))
        .and_then(|v| v.into_f64_vec())
    else {
        return 0.0;
    };
    scale[0]
}

pub fn parse_geotiff(path: &Path) -> Result<Heightmap, DemError> {
    let file: File = File::open(path)?;
    let mut decoder: Decoder<std::io::BufReader<File>> =
        Decoder::new(std::io::BufReader::new(file))?;
    // let mut decoder: Decoder<std::io::BufReader<File>> =
    //     Decoder::new(std::io::BufReader::new(file))?.with_limits(Limits::unlimited());

    let (cols, rows): (u32, u32) = decoder.dimensions()?;

    let scale = decoder.get_tag(Tag::Unknown(33550))?.into_f64_vec()?; // ModelPixelScaleTag
    // → Value::Double([sx, sy, sz])  — sx = deg/pixel in X, sy = deg/pixel in Y

    let tiepoint = decoder.get_tag(Tag::Unknown(33922))?.into_f64_vec()?; // ModelTiepointTag
    // → Value::Double([i, j, k, x, y, z])  — x = origin_lon, y = origin_lat

    let (dx_deg, dy_deg) = (scale[0], -scale[1]);
    let (origin_lon, origin_lat) = (tiepoint[3], tiepoint[4]);

    let img = decoder.read_image()?;
    let raw: Vec<f32> = match img {
        DecodingResult::F32(v) => v,
        _ => return Err("expected F32 image".into()),
    };

    // From the earlier analysis: NaN and values < -1000 are either tile padding or voids:
    const NODATA: f32 = -9999.0;
    let mut data: Vec<f32> = raw
        .iter()
        .map(|&v| if v.is_nan() || v < -1000.0 { NODATA } else { v })
        .collect();

    let before = data.iter().filter(|&&v| v == NODATA).count();
    fill_nodata(&mut data, rows as usize, cols as usize, NODATA);
    let after = data.iter().filter(|&&v| v == NODATA).count();
    println!("nodata cells — before: {}, after: {}", before, after);

    let min = data
        .iter()
        .cloned()
        .filter(|&v| v != NODATA)
        .fold(f32::INFINITY, f32::min);
    let max = data
        .iter()
        .cloned()
        .filter(|&v| v != NODATA)
        .fold(f32::NEG_INFINITY, f32::max);
    println!("elevation range check: {} to {} metres", min, max);

    let dy_meters = scale[1] * 111_320.0;
    let dx_meters = dx_deg * 111_320.0 * origin_lat.to_radians().cos();

    let heightmap: Heightmap = Heightmap {
        data: data,
        rows: rows as usize,
        cols: cols as usize,
        nodata: NODATA,
        origin_lat: origin_lat,
        origin_lon: origin_lon,
        dx_deg,
        dy_deg,
        dx_meters,
        dy_meters,
        crs_origin_x: origin_lon,
        crs_origin_y: origin_lat,
        crs_epsg: 4326,
    };

    Ok(heightmap)
}

pub fn parse_geotiff_epsg_31287(path: &Path) -> Result<Heightmap, DemError> {
    let file: File = File::open(path)?;
    let mut decoder: Decoder<std::io::BufReader<File>> =
        Decoder::new(std::io::BufReader::new(file))?;
    // let mut decoder: Decoder<std::io::BufReader<File>> =
    //     Decoder::new(std::io::BufReader::new(file))?.with_limits(Limits::unlimited());

    let (cols, rows): (u32, u32) = decoder.dimensions()?;

    let scale = decoder.get_tag(Tag::Unknown(33550))?.into_f64_vec()?; // ModelPixelScaleTag
    // → Value::Double([sx, sy, sz])  — sx = metres/pixel in X, sy = metres/pixel in Y

    let tiepoint = decoder.get_tag(Tag::Unknown(33922))?.into_f64_vec()?; // ModelTiepointTag
    // → Value::Double([i, j, k, x, y, z])  — x = easting, y = northing (metres, EPSG:31287)

    // Scale is already in metres — no degree→metre conversion needed.
    let dx_meters = scale[0];
    let dy_meters = scale[1];

    // tiepoint[3/4] are easting/northing in EPSG:31287 metres, not lon/lat degrees.
    // Approximate WGS84 from Austria Lambert false origin:
    //   false_easting=400000, false_northing=400000, central_meridian=13.333°, lat_origin=47.5°
    let origin_lat = 47.5 + (tiepoint[4] - 400_000.0) / 111_320.0;
    let origin_lon =
        13.333_333 + (tiepoint[3] - 400_000.0) / (111_320.0 * origin_lat.to_radians().cos());

    let img = decoder.read_image()?;
    let raw: Vec<f32> = match img {
        DecodingResult::F32(v) => v,
        _ => return Err("expected F32 image".into()),
    };

    // NoData for BEV DGM 5m is 0.0; minimum valid elevation in Austria is well above sea level.
    const NODATA: f32 = -9999.0;
    let mut data: Vec<f32> = raw
        .iter()
        .map(|&v| {
            if v == 0.0 || v.is_nan() || v < -1000.0 {
                NODATA
            } else {
                v
            }
        })
        .collect();

    let before = data.iter().filter(|&&v| v == NODATA).count();
    fill_nodata(&mut data, rows as usize, cols as usize, NODATA);
    let after = data.iter().filter(|&&v| v == NODATA).count();
    println!("nodata cells — before: {}, after: {}", before, after);

    let min = data
        .iter()
        .cloned()
        .filter(|&v| v != NODATA)
        .fold(f32::INFINITY, f32::min);
    let max = data
        .iter()
        .cloned()
        .filter(|&v| v != NODATA)
        .fold(f32::NEG_INFINITY, f32::max);
    println!("elevation range check: {} to {} metres", min, max);

    let heightmap: Heightmap = Heightmap {
        data,
        rows: rows as usize,
        cols: cols as usize,
        nodata: NODATA,
        origin_lat,
        origin_lon,
        dx_deg: 0.0,
        dy_deg: 0.0,
        dx_meters,
        dy_meters,
        crs_origin_x: tiepoint[3], // easting of top-left corner in EPSG:31287 metres
        crs_origin_y: tiepoint[4], // northing of top-left corner in EPSG:31287 metres
        crs_epsg: 31287,
    };

    Ok(heightmap)
}

pub fn parse_geotiff_epsg_3035(path: &Path) -> Result<Heightmap, DemError> {
    let file: File = File::open(path)?;
    // set no limits here to load big 1m resolution
    let mut decoder: Decoder<std::io::BufReader<File>> =
        Decoder::new(std::io::BufReader::new(file))?.with_limits(Limits::unlimited());

    let (cols, rows): (u32, u32) = decoder.dimensions()?;

    let scale = decoder.get_tag(Tag::Unknown(33550))?.into_f64_vec()?;
    let tiepoint = decoder.get_tag(Tag::Unknown(33922))?.into_f64_vec()?;

    let dx_meters = scale[0];
    let dy_meters = scale[1];

    // tiepoint[3/4] are easting/northing in EPSG:3035 metres.
    // Approximate WGS84 using spherical LAEA inverse (~100m accuracy, sufficient for sun direction).
    let (origin_lat, origin_lon) = laea_epsg3035_inverse(tiepoint[3], tiepoint[4]);

    let img = decoder.read_image()?;
    let raw: Vec<f32> = match img {
        DecodingResult::F32(v) => v,
        _ => return Err("expected F32 image".into()),
    };

    const NODATA: f32 = -9999.0;
    let mut data: Vec<f32> = raw
        .iter()
        .map(|&v| {
            if v == NODATA || v.is_nan() || v < -1000.0 {
                NODATA
            } else {
                v
            }
        })
        .collect();

    let before = data.iter().filter(|&&v| v == NODATA).count();
    fill_nodata(&mut data, rows as usize, cols as usize, NODATA);
    let after = data.iter().filter(|&&v| v == NODATA).count();
    println!("nodata cells — before: {}, after: {}", before, after);

    let min = data
        .iter()
        .cloned()
        .filter(|&v| v != NODATA)
        .fold(f32::INFINITY, f32::min);
    let max = data
        .iter()
        .cloned()
        .filter(|&v| v != NODATA)
        .fold(f32::NEG_INFINITY, f32::max);
    println!("elevation range check: {} to {} metres", min, max);

    Ok(Heightmap {
        data,
        rows: rows as usize,
        cols: cols as usize,
        nodata: NODATA,
        origin_lat,
        origin_lon,
        dx_deg: 0.0,
        dy_deg: 0.0,
        dx_meters,
        dy_meters,
        crs_origin_x: tiepoint[3],
        crs_origin_y: tiepoint[4],
        crs_epsg: 3035,
    })
}

/// Spherical LAEA inverse for EPSG:3035. Returns (lat_deg, lon_deg) in WGS84.
/// Accuracy ~100 m — sufficient for sun direction calculation.
fn laea_epsg3035_inverse(easting: f64, northing: f64) -> (f64, f64) {
    let r = 6_371_000.0_f64;
    let to_deg = 180.0 / std::f64::consts::PI;

    // EPSG:3035 parameters: lat0=52°N, lon0=10°E, FE=4321000, FN=3210000
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

    (lat * to_deg, lon * to_deg)
}
