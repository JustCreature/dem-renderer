use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use tiff::decoder::{Decoder, DecodingResult, Limits};
use tiff::tags::Tag;

use crate::heightmap::fill_nodata;
use crate::projection::Projection;
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

/// Parse a WGS84 geographic GeoTIFF (EPSG:4326, degrees/pixel).
pub fn parse_geotiff(path: &Path) -> Result<Heightmap, DemError> {
    let file: File = File::open(path)?;
    let mut decoder: Decoder<std::io::BufReader<File>> =
        Decoder::new(std::io::BufReader::new(file))?;

    let (cols, rows): (u32, u32) = decoder.dimensions()?;

    let scale = decoder.get_tag(Tag::Unknown(33550))?.into_f64_vec()?;
    let tiepoint = decoder.get_tag(Tag::Unknown(33922))?.into_f64_vec()?;

    let (dx_deg, dy_deg) = (scale[0], -scale[1]);
    let (origin_lon, origin_lat) = (tiepoint[3], tiepoint[4]);

    let img = decoder.read_image()?;
    let raw: Vec<f32> = match img {
        DecodingResult::F32(v) => v,
        _ => return Err("expected F32 image".into()),
    };

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

    Ok(Heightmap {
        data,
        rows: rows as usize,
        cols: cols as usize,
        nodata: NODATA,
        origin_lat,
        origin_lon,
        dx_deg,
        dy_deg,
        dx_meters,
        dy_meters,
        crs_origin_x: origin_lon,
        crs_origin_y: origin_lat,
        crs_epsg: 4326,
    })
}

/// Parse a projected GeoTIFF (metres/pixel) using the supplied projection for the inverse
/// tiepoint conversion (tiepoint → lat/lon for `origin_lat`/`origin_lon` fields).
pub fn parse_geotiff_projected(
    path: &Path,
    proj: &Arc<dyn Projection>,
) -> Result<Heightmap, DemError> {
    let file: File = File::open(path)?;
    let mut decoder: Decoder<std::io::BufReader<File>> =
        Decoder::new(std::io::BufReader::new(file))?.with_limits(Limits::unlimited());

    let (cols, rows): (u32, u32) = decoder.dimensions()?;

    let scale = decoder.get_tag(Tag::Unknown(33550))?.into_f64_vec()?;
    let tiepoint = decoder.get_tag(Tag::Unknown(33922))?.into_f64_vec()?;

    let dx_meters = scale[0];
    let dy_meters = scale[1];

    // tiepoint[3/4] are easting/northing in the projected CRS.
    let (origin_lat, origin_lon) = proj.inverse(tiepoint[3], tiepoint[4]);

    let img = decoder.read_image()?;
    let raw: Vec<f32> = match img {
        DecodingResult::F32(v) => v,
        _ => return Err("expected F32 image".into()),
    };

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

    // Carry the old crs_epsg field as 0 (unknown projected) — callers should use the
    // Arc<dyn Projection> they already hold rather than this number.
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
        crs_epsg: 0,
    })
}

/// Read a window from a projected COG centred on `centre_crs` (easting, northing in the
/// file's own CRS). `proj` converts those coordinates to lat/lon for the `origin_lat/lon` fields.
pub fn extract_window(
    path: &Path,
    centre_crs: (f64, f64),
    radius_m: f64,
    ifd_level: usize,
    proj: &Arc<dyn Projection>,
) -> Result<Heightmap, DemError> {
    let file: File = File::open(path)?;
    let mut decoder: Decoder<std::io::BufReader<File>> =
        Decoder::new(std::io::BufReader::new(file))?.with_limits(Limits::unlimited());

    // Geo-tags are stored only in IFD 0 for COG files.
    decoder.seek_to_image(0)?;

    let (full_cols, full_rows): (u32, u32) = decoder.dimensions()?;

    let scale = decoder.get_tag(Tag::Unknown(33550))?.into_f64_vec()?;
    let tiepoint = decoder.get_tag(Tag::Unknown(33922))?.into_f64_vec()?;

    let crs_origin_x = tiepoint[3];
    let crs_origin_y = tiepoint[4];

    decoder.seek_to_image(ifd_level)?;
    let (cols, rows): (u32, u32) = decoder.dimensions()?;

    let dx_meters = scale[0] * (full_cols as f64 / cols as f64);
    let dy_meters = scale[1] * (full_rows as f64 / rows as f64);

    let cx = (centre_crs.0 - crs_origin_x) / dx_meters;
    let cy = (crs_origin_y - centre_crs.1) / dy_meters;

    let (lat, lon) = proj.inverse(centre_crs.0, centre_crs.1);

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

    let (tw, th) = decoder.chunk_dimensions();
    let tiles_across = (cols as usize + tw as usize - 1) / tw as usize;

    let tc0 = px0 / tw as usize;
    let tc1 = (px1 + tw as usize - 1) / tw as usize;
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
            let tile_col0 = tc * tw as usize;
            let tile_row0 = tr * th as usize;
            let tile_col1 = (tile_col0 + tw as usize).min(cols as usize);
            let tile_row1 = (tile_row0 + th as usize).min(rows as usize);
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
        crs_epsg: 0,
    })
}
