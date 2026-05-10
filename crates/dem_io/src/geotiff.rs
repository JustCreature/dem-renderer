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

/// Read the projected CRS EPSG code directly from the GeoKey directory (tag 34735).
/// Looks for ProjectedCSTypeGeoKey (key ID 3072), whose inline value IS the EPSG code.
pub fn detect_projected_crs(path: &Path) -> Result<u32, String> {
    let file = File::open(path).map_err(|e| format!("cannot open {:?}: {e}", path))?;
    let mut decoder = Decoder::new(std::io::BufReader::new(file))
        .map_err(|e| format!("not a valid TIFF {:?}: {e}", path))?;
    let keys: Vec<u16> = decoder
        .get_tag(Tag::Unknown(34735))
        .and_then(|v| v.into_u16_vec())
        .map_err(|e| format!("GeoKeyDirectory tag missing in {:?}: {e}", path))?;
    // Layout: [version, key_rev, minor_rev, n_keys, then n_keys×4 shorts]
    let n = keys
        .get(3)
        .copied()
        .ok_or_else(|| format!("GeoKeyDirectory too short in {:?}", path))? as usize;
    for i in 0..n {
        let base = 4 + i * 4;
        let key_id = keys.get(base).copied().unwrap_or(0);
        let tiff_tag_location = keys.get(base + 1).copied().unwrap_or(0);
        let value_offset = keys.get(base + 3).copied().unwrap_or(0);
        if key_id == 3072 && tiff_tag_location == 0 {
            return Ok(value_offset as u32);
        }
    }
    Err(format!(
        "ProjectedCSTypeGeoKey (3072) not found in {:?}",
        path
    ))
}

/// Count how many IFD levels the file contains (IFD-0 = full res, IFD-1 = first overview, …).
/// Returns 1 on failure.
pub fn count_available_ifds(path: &Path) -> usize {
    let Ok(file) = File::open(path) else {
        return 1;
    };
    let Ok(mut decoder) =
        Decoder::new(std::io::BufReader::new(file)).map(|d| d.with_limits(Limits::unlimited()))
    else {
        return 1;
    };
    let mut count = 1usize;
    loop {
        if decoder.seek_to_image(count).is_ok() {
            count += 1;
        } else {
            break;
        }
    }
    count
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
    let (origin_lat, origin_lon) = laea_epsg31287_inverse(tiepoint[3], tiepoint[4]);

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

// Approximate WGS84 from Austria Lambert false origin:
// false_easting=400000, false_northing=400000, central_meridian=13.333°, lat_origin=47.5°
fn laea_epsg31287_inverse(easting: f64, northing: f64) -> (f64, f64) {
    let origin_lat = 47.5 + (northing - 400_000.0) / 111_320.0;
    let origin_lon =
        13.333_333 + (easting - 400_000.0) / (111_320.0 * origin_lat.to_radians().cos());

    (origin_lat, origin_lon)
}

pub fn extract_window(
    path: &Path,
    centre_crs: (f64, f64),
    radius_m: f64,
    ifd_level: usize,
    crs_epsg: u32,
) -> Result<Heightmap, DemError> {
    let file: File = File::open(path)?;
    // set no limits here to load big 1m resolution
    let mut decoder: Decoder<std::io::BufReader<File>> =
        Decoder::new(std::io::BufReader::new(file))?.with_limits(Limits::unlimited());

    // Geo-tags (ModelPixelScaleTag 33550, ModelTiepointTag 33922) are stored only in
    // IFD 0 for COG files; overview sub-IFDs do not repeat them.  Always read from IFD 0.
    decoder.seek_to_image(0)?;

    let (full_cols, full_rows): (u32, u32) = decoder.dimensions()?;

    let scale = decoder.get_tag(Tag::Unknown(33550))?.into_f64_vec()?; // ModelPixelScaleTag
    // → Value::Double([sx, sy, sz])  — sx = metres/pixel in X, sy = metres/pixel in Y

    let tiepoint = decoder.get_tag(Tag::Unknown(33922))?.into_f64_vec()?; // ModelTiepointTag
    // → Value::Double([i, j, k, x, y, z])  — x = easting, y = northing (metres, EPSG:31287)

    let crs_origin_x = tiepoint[3]; // easting of top-left corner in EPSG:31287 metres
    let crs_origin_y = tiepoint[4];

    // Seek to the requested IFD level for pixel data and actual dimensions.
    decoder.seek_to_image(ifd_level)?;
    let (cols, rows): (u32, u32) = decoder.dimensions()?;

    // Scale for this overview level: IFD-0 scale × (full_dim / ifd_dim).
    // Integer overview sizes may be ceil(full/2^n), so use the actual ratio instead of 2^n.
    let dx_meters = scale[0] * (full_cols as f64 / cols as f64);
    let dy_meters = scale[1] * (full_rows as f64 / rows as f64);

    let cx = (centre_crs.0 - crs_origin_x) / dx_meters;
    let cy = (crs_origin_y - centre_crs.1) / dy_meters;

    let (lat, lon) = match crs_epsg {
        3035 => laea_epsg3035_inverse(centre_crs.0, centre_crs.1),
        31287 => laea_epsg31287_inverse(centre_crs.0, centre_crs.1),
        v => panic!("not supporger geo format received: {:?}", v),
    };

    let radius_px_x = (radius_m / dx_meters) as isize;
    let radius_px_y = (radius_m / dy_meters) as isize;
    let px0 = (cx as isize - radius_px_x).max(0) as usize;
    let px1 = (cx as isize + radius_px_x).min(cols as isize) as usize;
    let py0 = (cy as isize - radius_px_y).max(0) as usize;
    let py1 = (cy as isize + radius_px_y).min(rows as isize) as usize;

    let out_w = px1 - px0;
    let out_h = py1 - py0;

    const NODATA: f32 = -9999.0;
    let mut data = vec![NODATA; out_w * out_h];

    let (tw, th) = decoder.chunk_dimensions(); // returns (u32, u32)                                                                                              
    let tiles_across = (cols as usize + tw as usize - 1) / tw as usize;

    let tc0 = px0 / tw as usize;
    let tc1 = (px1 + tw as usize - 1) / tw as usize; // exclusive, rounded up
    let tr0 = py0 / th as usize;
    let tr1 = (py1 + th as usize - 1) / th as usize;

    for tr in tr0..tr1 {
        for tc in tc0..tc1 {
            let index = (tr * tiles_across + tc) as u32;
            let chunk = decoder.read_chunk(index)?;
            let tile_data: Vec<f32> = match chunk {
                DecodingResult::F32(v) => v,
                _ => return Err("expected F32 tile".into()),
            };
            // overlap copy goes here
            let tile_col0 = tc * tw as usize;
            let tile_row0 = tr * th as usize;
            let tile_col1 = (tile_col0 + tw as usize).min(cols as usize);
            let tile_row1 = (tile_row0 + th as usize).min(rows as usize);
            // actual dims for edge tiles (last col/row may be narrower than tw/th)
            let actual_tw = tile_col1 - tile_col0;

            let col_start = tile_col0.max(px0);
            let col_end = tile_col1.min(px1);
            let row_start = tile_row0.max(py0);
            let row_end = tile_row1.min(py1);

            for row in row_start..row_end {
                let src = (row - tile_row0) * actual_tw + (col_start - tile_col0);
                let dst = (row - py0) * out_w + (col_start - px0);
                let len = col_end - col_start;
                for i in 0..len {
                    let v = tile_data[src + i];
                    // BEV DGM uses 0.0 as NoData sentinel instead of the SRTM convention
                    // (-32 768) or IEEE NaN.  This is safe because the minimum valid elevation
                    // in Austria is ~115 m (Hungarian border lowlands) — no real terrain pixel
                    // can be zero.  The three conditions cover:
                    //   v == 0.0    — BEV DGM NoData sentinel
                    //   v.is_nan()  — IEEE NaN from corrupt or partial tiles
                    //   v < -1000.0 — large-negative sentinel (SRTM-style, defensive guard)
                    data[dst + i] = if v == 0.0 || v.is_nan() || v < -1000.0 {
                        NODATA
                    } else {
                        v
                    };
                }
            }
        }
    }

    let win_crs_origin_x = crs_origin_x + px0 as f64 * dx_meters;
    let win_crs_origin_y = crs_origin_y - py0 as f64 * dy_meters;

    Ok(Heightmap {
        data,
        rows: out_h,
        cols: out_w,
        nodata: NODATA,
        origin_lat: lat,
        origin_lon: lon,
        dx_deg: 0.0,
        dy_deg: 0.0,
        dx_meters,
        dy_meters,
        crs_origin_x: win_crs_origin_x,
        crs_origin_y: win_crs_origin_y,
        crs_epsg,
    })
}
