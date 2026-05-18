use std::fs::File;
use std::path::Path;
use tiff::decoder::{Decoder, DecodingResult, Limits};
use tiff::tags::Tag;

use crate::crs;
use crate::heightmap::fill_nodata;
use crate::{DemError, Heightmap};

/// Returns ModelPixelScaleTag[0] from a GeoTIFF, or 0.0 on failure.
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

/// Parse any GeoTIFF into a Heightmap. CRS is read from the file and resolved
/// via proj4wkt (WKT from tag 34737) or crs-definitions (EPSG fallback).
/// No hardcoded CRS knowledge — works for any EPSG-registered projection.
pub fn parse_geotiff_auto(path: &Path) -> Result<Heightmap, DemError> {
    let crs_data = crs::read_geo_key_data(path)?;
    let proj4 = crs::proj4_from_keys(&crs_data)?;
    let epsg = crs_data
        .epsg
        .ok_or_else(|| DemError::from("no EPSG GeoKey found"))?;

    let file = File::open(path)?;
    let mut decoder = Decoder::new(std::io::BufReader::new(file))?.with_limits(Limits::unlimited());

    let (cols, rows) = decoder.dimensions()?;

    let scale = decoder.get_tag(Tag::Unknown(33550))?.into_f64_vec()?;
    let tiepoint = decoder.get_tag(Tag::Unknown(33922))?.into_f64_vec()?;

    let (origin_lat, origin_lon, dx_deg, dy_deg, dx_meters, dy_meters, crs_origin_x, crs_origin_y) =
        if crs::is_geographic(&proj4) {
            let origin_lon = tiepoint[3];
            let origin_lat = tiepoint[4];
            let dx_deg = scale[0];
            let dy_deg = scale[1];
            let dx_meters = dx_deg * 111_320.0 * origin_lat.to_radians().cos();
            let dy_meters = dy_deg * 111_320.0;
            (
                origin_lat, origin_lon, dx_deg, dy_deg, dx_meters, dy_meters, origin_lon,
                origin_lat,
            )
        } else {
            let dx_meters = scale[0];
            let dy_meters = scale[1];
            let (origin_lat, origin_lon) = crs::to_wgs84(tiepoint[3], tiepoint[4], &proj4)?;
            (
                origin_lat,
                origin_lon,
                0.0,
                0.0,
                dx_meters,
                dy_meters,
                tiepoint[3],
                tiepoint[4],
            )
        };

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
    println!("elevation range: {:.0}–{:.0} m", min, max);

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
        crs_origin_x,
        crs_origin_y,
        crs_epsg: epsg,
        crs_proj4: proj4,
    })
}

/// Return the WGS84 bounding box of a GeoTIFF tile without loading pixel data.
/// Returns `(lat_min, lat_max, lon_min, lon_max)`.
pub fn tile_bounds_wgs84(path: &Path) -> Result<(f64, f64, f64, f64), DemError> {
    let crs_data = crs::read_geo_key_data(path)?;
    let proj4 = crs::proj4_from_keys(&crs_data)?;

    let file = File::open(path)?;
    let mut decoder = Decoder::new(std::io::BufReader::new(file))?;
    let (cols, rows) = decoder.dimensions()?;
    let scale = decoder.get_tag(Tag::Unknown(33550))?.into_f64_vec()?;
    let tiepoint = decoder.get_tag(Tag::Unknown(33922))?.into_f64_vec()?;

    let origin_x = tiepoint[3];
    let origin_y = tiepoint[4];
    let dx = scale[0];
    let dy = scale[1];

    if crs::is_geographic(&proj4) {
        // origin is top-left corner in lon/lat degrees
        let lat_max = origin_y;
        let lat_min = origin_y - rows as f64 * dy;
        let lon_min = origin_x;
        let lon_max = origin_x + cols as f64 * dx;
        return Ok((lat_min, lat_max, lon_min, lon_max));
    }

    // Projected: convert all four corners via WGS84
    let (lat_tl, lon_tl) = crs::to_wgs84(origin_x, origin_y, &proj4)?;
    let (lat_tr, lon_tr) = crs::to_wgs84(origin_x + cols as f64 * dx, origin_y, &proj4)?;
    let (lat_bl, lon_bl) = crs::to_wgs84(origin_x, origin_y - rows as f64 * dy, &proj4)?;
    let (lat_br, lon_br) = crs::to_wgs84(
        origin_x + cols as f64 * dx,
        origin_y - rows as f64 * dy,
        &proj4,
    )?;

    let lat_min = lat_tl.min(lat_tr).min(lat_bl).min(lat_br);
    let lat_max = lat_tl.max(lat_tr).max(lat_bl).max(lat_br);
    let lon_min = lon_tl.min(lon_tr).min(lon_bl).min(lon_br);
    let lon_max = lon_tl.max(lon_tr).max(lon_bl).max(lon_br);
    Ok((lat_min, lat_max, lon_min, lon_max))
}

/// Returns the CRS-native (x, y) coordinate of the tile centre at IFD-0,
/// without loading image data.  x = easting or longitude, y = northing or latitude.
pub fn tile_centre_crs(path: &Path) -> Result<(f64, f64), DemError> {
    let file = File::open(path)?;
    let mut decoder = Decoder::new(std::io::BufReader::new(file))?;
    let (cols, rows) = decoder.dimensions()?;
    let scale = decoder.get_tag(Tag::Unknown(33550))?.into_f64_vec()?;
    let tiepoint = decoder.get_tag(Tag::Unknown(33922))?.into_f64_vec()?;
    Ok((
        tiepoint[3] + cols as f64 * scale[0] * 0.5,
        tiepoint[4] - rows as f64 * scale[1] * 0.5,
    ))
}

/// Returns the pixel scale (m/px or deg/px) at each IFD level in a GeoTIFF.
/// Index 0 = finest resolution (IFD-0), increasing index = coarser overviews.
pub fn ifd_scales(path: &Path) -> Result<Vec<f64>, DemError> {
    let file = File::open(path)?;
    let mut decoder = Decoder::new(std::io::BufReader::new(file))?.with_limits(Limits::unlimited());
    decoder.seek_to_image(0)?;
    let (full_cols, _) = decoder.dimensions()?;
    let base_scale = decoder.get_tag(Tag::Unknown(33550))?.into_f64_vec()?[0];
    let mut scales = vec![base_scale];
    let mut level = 1usize;
    loop {
        if decoder.seek_to_image(level).is_err() {
            break;
        }
        let (cols, _) = decoder.dimensions()?;
        scales.push(base_scale * (full_cols as f64 / cols as f64));
        level += 1;
    }
    Ok(scales)
}

pub fn extract_window(
    path: &Path,
    centre_crs: (f64, f64),
    radius_m: f64,
    ifd_level: usize,
) -> Result<Heightmap, DemError> {
    let crs_data = crs::read_geo_key_data(path)?;
    let proj4 = crs::proj4_from_keys(&crs_data)?;
    let crs_epsg = crs_data
        .epsg
        .ok_or_else(|| DemError::from("no EPSG GeoKey found"))?;

    let file: File = File::open(path)?;
    // set no limits here to load big 1m resolution
    let mut decoder: Decoder<std::io::BufReader<File>> =
        Decoder::new(std::io::BufReader::new(file))?.with_limits(Limits::unlimited());

    // Geo-tags are stored only in IFD 0 for COG files; overview sub-IFDs do not repeat them.
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

    let (lat, lon) = crs::to_wgs84(centre_crs.0, centre_crs.1, &proj4)?;

    let radius_px_x = (radius_m / dx_meters) as isize;
    let radius_px_y = (radius_m / dy_meters) as isize;
    // Keep as isize through the overlap check — casting to usize before the check causes
    // underflow (wrapping to 2^64) when the centre is completely outside the tile.
    let px0 = (cx as isize - radius_px_x).max(0);
    let px1 = (cx as isize + radius_px_x).min(cols as isize);
    let py0 = (cy as isize - radius_px_y).max(0);
    let py1 = (cy as isize + radius_px_y).min(rows as isize);

    if px1 <= px0 || py1 <= py0 {
        return Err("centre is outside tile bounds".into());
    }

    let px0 = px0 as usize;
    let px1 = px1 as usize;
    let py0 = py0 as usize;
    let py1 = py1 as usize;

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
        crs_proj4: proj4,
    })
}
