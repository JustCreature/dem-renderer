//! Camera-centred windows from orthophoto + land-cover mosaics.
//!
//! Reads a window from a 3-band JPEG-in-TIFF orthophoto (BEV mosaics store
//! YCbCr; the tiff crate's zune-jpeg path returns the samples unconverted, so
//! the BT.601 → RGB step happens here) and optionally drapes a single-band
//! land-cover raster over the same CRS rect, nearest-sampled onto the ortho
//! grid. Output is one RGBA8 buffer: RGB = albedo, A = material code — the
//! shader keys water/vegetation shading off A while a single texture binding
//! carries everything.

use std::fs::File;
use std::path::Path;

use tiff::ColorType;
use tiff::decoder::{Decoder, DecodingResult, Limits};
use tiff::tags::Tag;

use crate::crs;
use crate::{DemError, Heightmap};

/// Material codes packed into the alpha channel. Spaced apart so bilinear
/// filtering at class boundaries degrades into the neighbouring band instead
/// of aliasing to an unrelated material.
pub const MATERIAL_NONE: u8 = 0;
pub const MATERIAL_BUILDING: u8 = 64;
pub const MATERIAL_MED_VEG: u8 = 128;
pub const MATERIAL_HIGH_VEG: u8 = 192;
pub const MATERIAL_WATER: u8 = 255;

/// BEV Land Cover class → material code.
///
/// Class values pinned empirically with `cargo run -p dem_io --example
/// inspect_color` against `2022470_Mosaik_LC.tif` at single-class reference
/// spots: Achensee deep centre = 100% class 5 (water), Hintertux glacier =
/// 100% class 2 (ground/ice), Mayrhofen forest slope = 96% class 1 (high
/// vegetation), Uderns valley meadow = 88% class 6 (low vegetation), Mayrhofen
/// town = 44% class 4 (buildings). Class 3 is the remainder (medium
/// vegetation); 15 = NoData.
fn lc_code_to_material(code: u8) -> u8 {
    match code {
        1 => MATERIAL_HIGH_VEG,
        3 => MATERIAL_MED_VEG,
        4 => MATERIAL_BUILDING,
        5 => MATERIAL_WATER,
        _ => MATERIAL_NONE, // 2 = ground/ice, 6 = low veg/grass, 15 = NoData
    }
}

/// Result of `extract_color_window`.
pub struct ColorWindow {
    /// RGBA8, row-major: RGB = orthophoto albedo, A = material code.
    pub rgba: Vec<u8>,
    /// Georeferencing carrier for the rgba grid. `data` is intentionally empty —
    /// rows/cols/dx/dy/crs_* describe the window so the viewer can reuse the
    /// existing `Heightmap`-based placement code (`cross_crs_world_origin_and_extent`
    /// never touches `.data`).
    pub georef: Heightmap,
}

/// Raw single- or multi-sample u8 window read straight from one IFD level.
struct U8Window {
    data: Vec<u8>, // interleaved, `samples` per pixel
    samples: usize,
    cols: usize,
    rows: usize,
    dx: f64,
    dy: f64,
    origin_x: f64, // CRS coordinate of the window's top-left corner
    origin_y: f64,
    proj4: String,
    epsg: u32,
    color: ColorType,
}

/// Window-read core shared by the ortho and land-cover paths. Same geometry
/// contract as `geotiff::extract_window` (centre + radius in CRS units, clipped
/// to tile bounds, selective chunk reads at `ifd_level`) but for u8 rasters.
/// JPEG/LZW/Deflate chunks all go through the tiff crate's `read_chunk`.
fn extract_u8_window(
    path: &Path,
    centre_crs: (f64, f64),
    radius_m: f64,
    ifd_level: usize,
) -> Result<U8Window, DemError> {
    let crs_data = crs::read_geo_key_data(path)?;
    let proj4 = crs::proj4_from_keys(&crs_data)?;
    let epsg = crs_data
        .projected_epsg
        .or(crs_data.geographic_epsg)
        .unwrap_or(0);

    let file = File::open(path)?;
    let mut decoder = Decoder::new(std::io::BufReader::new(file))?.with_limits(Limits::unlimited());

    // Geo-tags live on IFD 0 only.
    decoder.seek_to_image(0)?;
    let (full_cols, full_rows): (u32, u32) = decoder.dimensions()?;
    let scale = decoder.get_tag(Tag::Unknown(33550))?.into_f64_vec()?;
    let tiepoint = decoder.get_tag(Tag::Unknown(33922))?.into_f64_vec()?;
    let crs_origin_x = tiepoint[3];
    let crs_origin_y = tiepoint[4];

    decoder.seek_to_image(ifd_level)?;
    let (cols, rows): (u32, u32) = decoder.dimensions()?;
    let dx = scale[0] * (full_cols as f64 / cols as f64);
    let dy = scale[1] * (full_rows as f64 / rows as f64);

    let color = decoder.colortype()?;
    // (samples per pixel, bits per sample). 4-bit grayscale is what BEV's
    // land-cover mosaic uses (6 classes + NoData fit a nibble); the tiff crate
    // returns those chunks bit-packed two pixels per byte, MSB nibble first.
    let (samples, bits) = match color {
        ColorType::Gray(8) => (1, 8),
        ColorType::Gray(4) => (1, 4),
        ColorType::RGB(8) | ColorType::YCbCr(8) => (3, 8),
        ColorType::RGBA(8) => (4, 8),
        other => return Err(format!("unsupported color raster type {other:?}").into()),
    };

    let cx = (centre_crs.0 - crs_origin_x) / dx;
    let cy = (crs_origin_y - centre_crs.1) / dy;
    let radius_px_x = (radius_m / dx) as isize;
    let radius_px_y = (radius_m / dy) as isize;
    let px0 = (cx as isize - radius_px_x).max(0);
    let px1 = (cx as isize + radius_px_x).min(cols as isize);
    let py0 = (cy as isize - radius_px_y).max(0);
    let py1 = (cy as isize + radius_px_y).min(rows as isize);
    if px1 <= px0 || py1 <= py0 {
        return Err("centre is outside tile bounds".into());
    }
    let (px0, px1, py0, py1) = (px0 as usize, px1 as usize, py0 as usize, py1 as usize);
    let out_w = px1 - px0;
    let out_h = py1 - py0;

    let mut data = vec![0u8; out_w * out_h * samples];

    let (tw, th) = decoder.chunk_dimensions();
    let tiles_across = (cols as usize).div_ceil(tw as usize);
    let tc0 = px0 / tw as usize;
    let tc1 = px1.div_ceil(tw as usize);
    let tr0 = py0 / th as usize;
    let tr1 = py1.div_ceil(th as usize);

    for tr in tr0..tr1 {
        for tc in tc0..tc1 {
            let index = (tr * tiles_across + tc) as u32;
            let tile_col0 = tc * tw as usize;
            let tile_row0 = tr * th as usize;
            let tile_col1 = (tile_col0 + tw as usize).min(cols as usize);
            let tile_row1 = (tile_row0 + th as usize).min(rows as usize);
            // read_chunk trims edge tiles to their actual data dimensions.
            let actual_tw = tile_col1 - tile_col0;
            let actual_th = tile_row1 - tile_row0;

            let tile_data: Vec<u8> = match decoder.read_chunk(index)? {
                DecodingResult::U8(v) => v,
                _ => return Err("expected U8 chunk in color raster".into()),
            };
            // Unpack 4-bit chunks to one sample per byte so the copy loop below
            // can stay sample-addressed. Rows are byte-aligned in the packed form.
            let tile_data: Vec<u8> = if bits == 4 {
                let row_bytes = actual_tw.div_ceil(2);
                let mut unpacked = vec![0u8; actual_tw * actual_th];
                for r in 0..actual_th {
                    for c in 0..actual_tw {
                        let byte = tile_data[r * row_bytes + c / 2];
                        unpacked[r * actual_tw + c] =
                            if c % 2 == 0 { byte >> 4 } else { byte & 0x0F };
                    }
                }
                unpacked
            } else {
                tile_data
            };

            let col_start = tile_col0.max(px0);
            let col_end = tile_col1.min(px1);
            let row_start = tile_row0.max(py0);
            let row_end = tile_row1.min(py1);

            for row in row_start..row_end {
                let src = ((row - tile_row0) * actual_tw + (col_start - tile_col0)) * samples;
                let dst = ((row - py0) * out_w + (col_start - px0)) * samples;
                let len = (col_end - col_start) * samples;
                data[dst..dst + len].copy_from_slice(&tile_data[src..src + len]);
            }
        }
    }

    Ok(U8Window {
        data,
        samples,
        cols: out_w,
        rows: out_h,
        dx,
        dy,
        origin_x: crs_origin_x + px0 as f64 * dx,
        origin_y: crs_origin_y - py0 as f64 * dy,
        proj4,
        epsg,
        color,
    })
}

/// BT.601 full-range YCbCr → RGB (the JPEG convention).
#[inline]
pub(crate) fn ycbcr_to_rgb(y: u8, cb: u8, cr: u8) -> [u8; 3] {
    let y = y as f32;
    let cb = cb as f32 - 128.0;
    let cr = cr as f32 - 128.0;
    let r = y + 1.402 * cr;
    let g = y - 0.344_136 * cb - 0.714_136 * cr;
    let b = y + 1.772 * cb;
    [
        r.clamp(0.0, 255.0) as u8,
        g.clamp(0.0, 255.0) as u8,
        b.clamp(0.0, 255.0) as u8,
    ]
}

/// Extract a camera-centred RGBA8 window: RGB albedo from `rgb_path` at
/// `rgb_ifd`, material codes in A from `lc_path` at `lc_ifd` (nearest-sampled
/// onto the ortho grid by CRS coordinate; A = 0 everywhere when no land cover
/// is supplied or a pixel falls outside its coverage).
///
/// `centre_crs` is in the ortho file's CRS; callers convert from WGS84 via
/// `crs::from_wgs84` exactly like the height-tier workers do.
pub fn extract_color_window(
    rgb_path: &Path,
    lc_path: Option<&Path>,
    centre_crs: (f64, f64),
    radius_m: f64,
    rgb_ifd: usize,
    lc_ifd: Option<usize>,
) -> Result<ColorWindow, DemError> {
    let rgb = extract_u8_window(rgb_path, centre_crs, radius_m, rgb_ifd)?;
    if rgb.samples != 3 {
        return Err(format!("ortho must be 3-band, got {} samples", rgb.samples).into());
    }
    let needs_ycbcr = matches!(rgb.color, ColorType::YCbCr(_));

    let n_px = rgb.cols * rgb.rows;
    let mut rgba = vec![0u8; n_px * 4];
    for i in 0..n_px {
        let s = &rgb.data[i * 3..i * 3 + 3];
        let [r, g, b] = if needs_ycbcr {
            ycbcr_to_rgb(s[0], s[1], s[2])
        } else {
            [s[0], s[1], s[2]]
        };
        rgba[i * 4] = r;
        rgba[i * 4 + 1] = g;
        rgba[i * 4 + 2] = b;
        // alpha stays MATERIAL_NONE until the land-cover pass below
    }

    if let Some(lc_path) = lc_path {
        match extract_u8_window(lc_path, centre_crs, radius_m, lc_ifd.unwrap_or(0)) {
            Ok(lc) if lc.samples == 1 => {
                for r in 0..rgb.rows {
                    let y = rgb.origin_y - (r as f64 + 0.5) * rgb.dy;
                    let lc_r = ((lc.origin_y - y) / lc.dy) as isize;
                    if lc_r < 0 || lc_r >= lc.rows as isize {
                        continue;
                    }
                    for c in 0..rgb.cols {
                        let x = rgb.origin_x + (c as f64 + 0.5) * rgb.dx;
                        let lc_c = ((x - lc.origin_x) / lc.dx) as isize;
                        if lc_c < 0 || lc_c >= lc.cols as isize {
                            continue;
                        }
                        let code = lc.data[lc_r as usize * lc.cols + lc_c as usize];
                        rgba[(r * rgb.cols + c) * 4 + 3] = lc_code_to_material(code);
                    }
                }
            }
            Ok(lc) => eprintln!(
                "[color] land cover {} has {} samples/px, expected 1 — skipping",
                lc_path.display(),
                lc.samples
            ),
            Err(e) => eprintln!(
                "[color] land cover window failed ({e}) — continuing without material codes"
            ),
        }
    }

    Ok(ColorWindow {
        rgba,
        georef: Heightmap {
            data: Vec::new(),
            rows: rgb.rows,
            cols: rgb.cols,
            nodata: -9999.0,
            origin_lat: 0.0,
            origin_lon: 0.0,
            dx_deg: 0.0,
            dy_deg: 0.0,
            dx_meters: rgb.dx,
            dy_meters: rgb.dy,
            crs_origin_x: rgb.origin_x,
            crs_origin_y: rgb.origin_y,
            crs_epsg: rgb.epsg,
            crs_proj4: rgb.proj4,
        },
    })
}

/// Histogram of land-cover class values over a window — backing for the
/// `inspect_color` example that pins `lc_code_to_material`.
pub fn landcover_histogram(
    lc_path: &Path,
    centre_crs: (f64, f64),
    radius_m: f64,
    ifd: usize,
) -> Result<[u64; 256], DemError> {
    let win = extract_u8_window(lc_path, centre_crs, radius_m, ifd)?;
    if win.samples != 1 {
        return Err("land cover must be single-band".into());
    }
    let mut hist = [0u64; 256];
    for &v in &win.data {
        hist[v as usize] += 1;
    }
    Ok(hist)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ycbcr_known_vectors() {
        // Neutral grays: Cb = Cr = 128 must pass Y through unchanged.
        assert_eq!(ycbcr_to_rgb(0, 128, 128), [0, 0, 0]);
        assert_eq!(ycbcr_to_rgb(255, 128, 128), [255, 255, 255]);
        assert_eq!(ycbcr_to_rgb(99, 128, 128), [99, 99, 99]);
        // Primary red (255,0,0) encodes to Y≈76, Cb≈84.4, Cr=255 in BT.601.
        let [r, g, b] = ycbcr_to_rgb(76, 84, 255);
        assert!(r >= 250, "red channel saturates, got {r}");
        assert!(g <= 10 && b <= 10, "green/blue near zero, got {g}/{b}");
        // Primary blue (0,0,255) → Y≈29, Cb=255, Cr≈107.
        let [r, g, b] = ycbcr_to_rgb(29, 255, 107);
        assert!(b >= 250, "blue channel saturates, got {b}");
        assert!(r <= 10 && g <= 35, "red/green low, got {r}/{g}");
    }

    #[test]
    fn material_ladder_is_widely_spaced() {
        // Bilinear filtering mixes adjacent texels; codes must stay ≥ 64 apart
        // so a 50/50 boundary mix never lands inside another class's band.
        let codes = [
            MATERIAL_NONE,
            MATERIAL_BUILDING,
            MATERIAL_MED_VEG,
            MATERIAL_HIGH_VEG,
            MATERIAL_WATER,
        ];
        for pair in codes.windows(2) {
            assert!(pair[1] - pair[0] >= 63, "codes too close: {pair:?}");
        }
        assert_eq!(lc_code_to_material(15), MATERIAL_NONE, "LC NoData → none");
    }
}
