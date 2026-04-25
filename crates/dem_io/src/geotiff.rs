use std::fs::File;
use std::path::Path;
use tiff::decoder::{Decoder, DecodingResult};
use tiff::tags::Tag;

use crate::heightmap::fill_nodata;
use crate::{DemError, Heightmap};

/// Returns true if the GeoTIFF uses a projected CRS (scale in metres, e.g. EPSG:31287).
/// Heuristic: ModelPixelScaleTag[0] > 1.0 means metres/pixel; < 0.1 means degrees/pixel.
pub fn geotiff_is_projected(path: &Path) -> bool {
    let Ok(file) = File::open(path) else { return false };
    let Ok(mut decoder) = Decoder::new(std::io::BufReader::new(file)) else { return false };
    let Ok(scale) = decoder.get_tag(Tag::Unknown(33550)).and_then(|v| v.into_f64_vec()) else {
        return false;
    };
    scale[0] > 1.0
}

pub fn parse_geotiff(path: &Path) -> Result<Heightmap, DemError> {
    let file: File = File::open(path)?;
    let mut decoder: Decoder<std::io::BufReader<File>> =
        Decoder::new(std::io::BufReader::new(file))?;

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
    };

    Ok(heightmap)
}

pub fn parse_geotiff_epsg_31287(path: &Path) -> Result<Heightmap, DemError> {
    let file: File = File::open(path)?;
    let mut decoder: Decoder<std::io::BufReader<File>> =
        Decoder::new(std::io::BufReader::new(file))?;

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
    let origin_lon = 13.333_333
        + (tiepoint[3] - 400_000.0) / (111_320.0 * origin_lat.to_radians().cos());

    let img = decoder.read_image()?;
    let raw: Vec<f32> = match img {
        DecodingResult::F32(v) => v,
        _ => return Err("expected F32 image".into()),
    };

    // NoData for BEV DGM 5m is 0.0; minimum valid elevation in Austria is well above sea level.
    const NODATA: f32 = -9999.0;
    let mut data: Vec<f32> = raw
        .iter()
        .map(|&v| if v == 0.0 || v.is_nan() || v < -1000.0 { NODATA } else { v })
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
    };

    Ok(heightmap)
}
