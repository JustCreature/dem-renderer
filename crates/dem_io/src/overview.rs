use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use tiff::decoder::{Decoder, DecodingResult, Limits};
use tiff::encoder::{TiffEncoder, colortype};
use tiff::tags::Tag;

use crate::crs;
use crate::lzw_lenient;
use crate::{DemError, Heightmap};

const NODATA: f32 = -9999.0;

/// Target pixel scale (m/px) for the close-range overview IFD (used when source < 5 m/px).
pub const CLOSE_OVERVIEW_TARGET_M: f64 = 8.0;
/// Target pixel scale (m/px) for the base overview IFD.
pub const BASE_OVERVIEW_TARGET_M: f64 = 32.0;

/// Read the source GeoTIFF chunk-by-chunk and produce one box-averaged `Heightmap`
/// per entry in `factors` (ascending order, e.g. `[8, 32]`).
/// Each input chunk is read exactly once; peak extra RAM = one tile buffer + all
/// output accumulators (never the full source image).
pub(crate) fn build_downsampled(
    src_path: &Path,
    factors: &[usize],
    report: impl Fn(f32),
) -> Result<Vec<Heightmap>, DemError> {
    let crs_data = crs::read_geo_key_data(src_path)?;
    let proj4 = crs::proj4_from_keys(&crs_data)?;
    let epsg = crs_data
        .projected_epsg
        .or(crs_data.geographic_epsg)
        .unwrap_or(0);

    let file = File::open(src_path)?;
    let mut decoder = Decoder::new(std::io::BufReader::new(file))?.with_limits(Limits::unlimited());
    decoder.seek_to_image(0)?;

    let (full_cols_u32, full_rows_u32) = decoder.dimensions()?;
    let full_cols = full_cols_u32 as usize;
    let full_rows = full_rows_u32 as usize;

    let scale_tag = decoder.get_tag(Tag::Unknown(33550))?.into_f64_vec()?;
    let tiepoint = decoder.get_tag(Tag::Unknown(33922))?.into_f64_vec()?;

    let src_dx = scale_tag[0];
    let src_dy = scale_tag[1];
    let crs_origin_x = tiepoint[3];
    let crs_origin_y = tiepoint[4];
    let gdal_nodata = crate::geotiff::read_gdal_nodata(&mut decoder);

    let is_geo = crs::is_geographic(&proj4);
    let (origin_lat, origin_lon) = if is_geo {
        (crs_origin_y, crs_origin_x)
    } else {
        crs::to_wgs84(crs_origin_x, crs_origin_y, &proj4)?
    };

    // Allocate sum + count accumulators for every factor level simultaneously.
    let mut sums: Vec<Vec<f64>> = Vec::with_capacity(factors.len());
    let mut cnts: Vec<Vec<u32>> = Vec::with_capacity(factors.len());
    let mut out_dims: Vec<(usize, usize)> = Vec::with_capacity(factors.len());
    for &f in factors {
        let out_r = full_rows.div_ceil(f);
        let out_c = full_cols.div_ceil(f);
        sums.push(vec![0.0f64; out_r * out_c]);
        cnts.push(vec![0u32; out_r * out_c]);
        out_dims.push((out_r, out_c));
    }

    // One pass through every chunk in the source file — each chunk read exactly once.
    let (tw_u32, th_u32) = decoder.chunk_dimensions();
    let tw = tw_u32 as usize;
    let th = th_u32 as usize;
    let tiles_across = full_cols.div_ceil(tw);
    let tiles_down = full_rows.div_ceil(th);
    let total = (tiles_across * tiles_down) as f32;

    // Detect LZW once.  LZW chunks go through the lenient reader (issue #40 —
    // the tiff crate's strict EOI requirement crashes on certain LZW tiles on
    // Windows).  Other compressions stay on the existing fast path.
    let (compression, predictor) = lzw_lenient::compression_and_predictor(&mut decoder)?;
    let (chunk_offsets, chunk_byte_counts, mut lzw_file, lzw_byte_order, lzw_is_tiled) =
        if compression == lzw_lenient::COMPRESSION_LZW {
            let (offs, counts, is_tiled) = lzw_lenient::chunk_layout(&mut decoder)?;
            let mut f = File::open(src_path)?;
            let order = lzw_lenient::read_byte_order(&mut f)?;
            (offs, counts, Some(f), order, is_tiled)
        } else {
            (
                Vec::new(),
                Vec::new(),
                None,
                lzw_lenient::SampleByteOrder::LittleEndian,
                false,
            )
        };

    for tr in 0..tiles_down {
        report(tr as f32 * tiles_across as f32 / total);
        for tc in 0..tiles_across {
            let index = (tr * tiles_across + tc) as u32;
            let tile_col0 = tc * tw;
            let tile_row0 = tr * th;
            let tile_col1 = (tile_col0 + tw).min(full_cols);
            let tile_row1 = (tile_row0 + th).min(full_rows);
            // Edge tiles produce actual_tw × actual_th data; matches both the
            // tiff crate's read_chunk convention and the lenient reader's trim.
            let actual_tw = tile_col1 - tile_col0;
            let actual_th = tile_row1 - tile_row0;

            let tile_data: Vec<f32> = if let Some(file) = lzw_file.as_mut() {
                let i = index as usize;
                // Tiled TIFFs encode the full padded TileLength even for edge
                // tiles; stripped TIFFs encode only the rows that actually fit.
                let encoded_rows = if lzw_is_tiled { th } else { actual_th };
                lzw_lenient::read_lzw_chunk_f32(
                    file,
                    chunk_offsets[i],
                    chunk_byte_counts[i] as usize,
                    tw,
                    encoded_rows,
                    actual_tw,
                    actual_th,
                    predictor,
                    lzw_byte_order,
                )?
            } else {
                match decoder.read_chunk(index)? {
                    DecodingResult::F32(v) => v,
                    _ => return Err("expected F32 tile in source GeoTIFF".into()),
                }
            };

            for local_r in 0..(tile_row1 - tile_row0) {
                let gr = tile_row0 + local_r;
                for local_c in 0..(tile_col1 - tile_col0) {
                    let gc = tile_col0 + local_c;
                    let v = tile_data[local_r * actual_tw + local_c];
                    if crate::geotiff::is_nodata_value(v, gdal_nodata) {
                        continue;
                    }
                    // Scatter this pixel into every downsample level simultaneously.
                    for (fi, &f) in factors.iter().enumerate() {
                        let out_r = gr / f;
                        let out_c = gc / f;
                        let out_cols = out_dims[fi].1;
                        let idx = out_r * out_cols + out_c;
                        sums[fi][idx] += v as f64;
                        cnts[fi][idx] += 1;
                    }
                }
            }
        }
    }

    // Finalise: average accumulators → Heightmap per factor.
    let mut results = Vec::with_capacity(factors.len());
    for (fi, &f) in factors.iter().enumerate() {
        let (out_rows, out_cols) = out_dims[fi];
        let data: Vec<f32> = (0..out_rows * out_cols)
            .map(|i| {
                if cnts[fi][i] > 0 {
                    (sums[fi][i] / cnts[fi][i] as f64) as f32
                } else {
                    NODATA
                }
            })
            .collect();

        let (dx_meters, dy_meters, dx_deg, dy_deg) = if is_geo {
            (
                src_dx * f as f64 * 111_320.0 * origin_lat.to_radians().cos(),
                src_dy * f as f64 * 111_320.0,
                src_dx * f as f64,
                src_dy * f as f64,
            )
        } else {
            (src_dx * f as f64, src_dy * f as f64, 0.0, 0.0)
        };

        results.push(Heightmap {
            data,
            rows: out_rows,
            cols: out_cols,
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
            crs_proj4: proj4.clone(),
        });
    }

    Ok(results)
}

/// Write `heightmaps` (finest first) as a multi-IFD Float32 GeoTIFF.
/// IFD-0 gets full geo-tags: 33550 (pixel scale), 33922 (tiepoint), and the
/// three CRS-defining tags (34735, 34736, 34737) copied verbatim from `src_crs`.
/// Verbatim copy is what keeps the cache self-describing for files whose CRS
/// is not a simple EPSG code (HMA's user-defined Albers etc.).
/// Subsequent IFDs carry image data only — matching the COG convention that
/// `extract_window` / `ifd_scales` rely on. Writes atomically via a `.partial` rename.
fn write_overview_tiff(
    dst_path: &Path,
    heightmaps: &[&Heightmap],
    src_crs: &crs::RawCrsTags,
) -> Result<(), DemError> {
    let partial = dst_path.with_extension("partial");

    // tiff crate stores Short-typed tag entries as u32. Cast back to u16 for writing.
    let geo_key_dir_u16: Vec<u16> = src_crs
        .geo_key_directory
        .iter()
        .map(|&v| v as u16)
        .collect();

    {
        let file = File::create(&partial)?;
        let mut encoder = TiffEncoder::new(BufWriter::new(file))?;

        for (idx, hm) in heightmaps.iter().enumerate() {
            let mut img =
                encoder.new_image::<colortype::Gray32Float>(hm.cols as u32, hm.rows as u32)?;

            if idx == 0 {
                img.encoder()
                    .write_tag(Tag::Unknown(33550), [hm.dx_meters, hm.dy_meters, 0.0_f64])?;
                img.encoder().write_tag(
                    Tag::Unknown(33922),
                    [0.0_f64, 0.0, 0.0, hm.crs_origin_x, hm.crs_origin_y, 0.0],
                )?;
                img.encoder()
                    .write_tag(Tag::Unknown(34735), geo_key_dir_u16.as_slice())?;
                if !src_crs.geo_double_params.is_empty() {
                    img.encoder()
                        .write_tag(Tag::Unknown(34736), src_crs.geo_double_params.as_slice())?;
                }
                if let Some(ref ascii) = src_crs.geo_ascii_params {
                    img.encoder()
                        .write_tag(Tag::Unknown(34737), ascii.as_str())?;
                }
            }

            img.write_data(hm.data.as_slice())?;
        }
    } // file flushed and closed before rename

    std::fs::rename(&partial, dst_path)?;
    Ok(())
}

/// Check whether `tile_path` needs a pre-computed overview cache; generate it
/// if needed and return the cache path.  Returns `None` when the tile already
/// has overviews, is coarse enough on its own, or cache generation fails.
///
/// The cache is a `.tmp_dem_pre_calc_<filename>` GeoTIFF written next to the
/// source tile.  On subsequent runs the file is reused as long as it is not
/// older than the source tile.
///
/// Should only be called for projected-CRS single-IFD tiles; the caller is
/// responsible for the `is_projected` guard.
pub fn ensure_overview_cache(
    tile_path: &Path,
    report: impl Fn(f32, &str),
) -> Result<Option<PathBuf>, DemError> {
    let scales = match crate::ifd_scales(tile_path) {
        Ok(s) => s,
        Err(_) => return Ok(None),
    };

    // No cache needed: tile already has multiple IFD levels, or is coarse enough
    // that base-tier select_ifd (≥ 30 m) can pick it directly.
    if scales.len() > 1 || scales[0] >= 20.0 {
        return Ok(None);
    }

    let src_scale = scales[0];

    let tile_name = tile_path
        .file_name()
        .ok_or("tile path has no filename")?
        .to_string_lossy();
    let cache_name = format!(".tmp_dem_pre_calc_{tile_name}");
    let cache_path = tile_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(&cache_name);

    // Cache hit: exists and at least as recent as the source file.
    if let (Ok(cache_meta), Ok(src_meta)) =
        (std::fs::metadata(&cache_path), std::fs::metadata(tile_path))
        && let (Ok(cache_mtime), Ok(src_mtime)) = (cache_meta.modified(), src_meta.modified())
        && cache_mtime >= src_mtime
    {
        println!("overview cache hit: {cache_name}");
        return Ok(Some(cache_path));
    }

    // Determine downsample factors:
    //   close tier → target ~8 m/px  (only for sub-5m sources)
    //   base  tier → target ~32 m/px
    let base_factor = ((BASE_OVERVIEW_TARGET_M / src_scale).round() as usize).max(2);
    let factors: Vec<usize> = if src_scale < 5.0 {
        let close_factor = ((CLOSE_OVERVIEW_TARGET_M / src_scale).round() as usize).max(2);
        vec![close_factor, base_factor]
    } else {
        vec![base_factor]
    };

    let label: Vec<String> = factors
        .iter()
        .map(|&f| format!("{:.0}m", src_scale * f as f64))
        .collect();
    let label = label.join("+");

    report(0.0, &format!("Building overview cache ({label})…"));

    let overviews = build_downsampled(tile_path, &factors, |p| {
        report(p * 0.9, &format!("Building overview cache ({label})…"));
    })?;

    println!(
        "overview built: {} → {} IFD(s) [{label}]",
        tile_name,
        overviews.len()
    );

    report(0.9, "Writing overview cache to disk…");
    let src_crs = crs::read_raw_crs_tags(tile_path)?;
    let refs: Vec<&Heightmap> = overviews.iter().collect();
    if let Err(e) = write_overview_tiff(&cache_path, &refs, &src_crs) {
        eprintln!("warning: could not write overview cache ({e}); falling back to slow path");
        return Ok(None);
    }

    report(1.0, "Overview cache ready");
    println!("overview cache written: {cache_name}");
    Ok(Some(cache_path))
}
