//! Synthetic round-trip tests over GeoTIFFs written in-process.
//!
//! These validate the general read/cache mechanism without depending on any real
//! fixture's quirks: geographic vs projected scale derivation, and the overview
//! cache build → reopen → CRS-preservation cycle. The tiny GeoTIFF writer mirrors
//! the tag layout `overview.rs::write_overview_tiff` uses.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use dem_io::{crs, ensure_overview_cache, ifd_scales, parse_geotiff_auto};
use tiff::encoder::{TiffEncoder, colortype};
use tiff::tags::Tag;

/// Write a minimal single-IFD Float32 GeoTIFF with the geo-tags the reader needs:
/// 33550 (pixel scale), 33922 (tiepoint) and 34735 (GeoKeyDirectory).
fn write_min_geotiff(
    path: &Path,
    cols: u32,
    rows: u32,
    scale: (f64, f64),
    origin: (f64, f64),
    geo_key_dir: &[u16],
    data: &[f32],
) {
    let file = File::create(path).unwrap();
    let mut enc = TiffEncoder::new(BufWriter::new(file)).unwrap();
    let mut img = enc.new_image::<colortype::Gray32Float>(cols, rows).unwrap();
    img.encoder()
        .write_tag(Tag::Unknown(33550), &[scale.0, scale.1, 0.0_f64][..])
        .unwrap();
    img.encoder()
        .write_tag(
            Tag::Unknown(33922),
            &[0.0_f64, 0.0, 0.0, origin.0, origin.1, 0.0][..],
        )
        .unwrap();
    img.encoder()
        .write_tag(Tag::Unknown(34735), geo_key_dir)
        .unwrap();
    img.write_data(data).unwrap();
}

/// Like `write_min_geotiff` but also writes the GDAL_NODATA ASCII tag (42113),
/// the way GDAL/opals declare per-file sentinels (e.g. the BEV ALS DSM's +3.4e38).
#[allow(clippy::too_many_arguments)]
fn write_min_geotiff_with_nodata(
    path: &Path,
    cols: u32,
    rows: u32,
    scale: (f64, f64),
    origin: (f64, f64),
    geo_key_dir: &[u16],
    data: &[f32],
    nodata: &str,
) {
    let file = File::create(path).unwrap();
    let mut enc = TiffEncoder::new(BufWriter::new(file)).unwrap();
    let mut img = enc.new_image::<colortype::Gray32Float>(cols, rows).unwrap();
    img.encoder()
        .write_tag(Tag::Unknown(33550), &[scale.0, scale.1, 0.0_f64][..])
        .unwrap();
    img.encoder()
        .write_tag(
            Tag::Unknown(33922),
            &[0.0_f64, 0.0, 0.0, origin.0, origin.1, 0.0][..],
        )
        .unwrap();
    img.encoder()
        .write_tag(Tag::Unknown(34735), geo_key_dir)
        .unwrap();
    img.encoder()
        .write_tag(Tag::Unknown(42113), nodata)
        .unwrap();
    img.write_data(data).unwrap();
}

/// GeoKeyDirectory for a projected CRS given by EPSG (3072).
fn projected_dir(epsg: u16) -> Vec<u16> {
    // header: version 1.1.0, 2 keys; GTModelType=1 (projected); ProjectedCSType=epsg
    vec![1, 1, 0, 2, 1024, 0, 1, 1, 3072, 0, 1, epsg]
}

/// GeoKeyDirectory for a geographic CRS given by EPSG (2048).
fn geographic_dir(epsg: u16) -> Vec<u16> {
    // header: version 1.1.0, 2 keys; GTModelType=2 (geographic); GeographicType=epsg
    vec![1, 1, 0, 2, 1024, 0, 1, 2, 2048, 0, 1, epsg]
}

fn ramp(cols: u32, rows: u32) -> Vec<f32> {
    (0..rows * cols)
        .map(|i| 500.0 + (i % 97) as f32) // finite, plausible elevations
        .collect()
}

#[test]
fn projected_and_geographic_scales_are_derived_differently() {
    let dir = tempfile::tempdir().unwrap();

    // Projected (EPSG:3035): dx_meters comes straight from the pixel scale,
    // dx_deg stays 0.
    let proj_path = dir.path().join("proj.tif");
    write_min_geotiff(
        &proj_path,
        8,
        8,
        (10.0, 10.0),
        (4_400_000.0, 2_700_000.0),
        &projected_dir(3035),
        &ramp(8, 8),
    );
    let p = parse_geotiff_auto(&proj_path).unwrap();
    assert!(!crs::is_geographic(&p.crs_proj4));
    assert_eq!(p.dx_meters, 10.0, "projected dx_meters = scale[0]");
    assert_eq!(p.dx_deg, 0.0, "projected dx_deg must be 0");

    // Geographic (EPSG:4326): dx_deg is the raw degree scale, dx_meters derived
    // via cos(lat).
    let geo_path = dir.path().join("geo.tif");
    let dscale = 0.000_277_777_777_778;
    write_min_geotiff(
        &geo_path,
        8,
        8,
        (dscale, dscale),
        (11.0, 47.0),
        &geographic_dir(4326),
        &ramp(8, 8),
    );
    let g = parse_geotiff_auto(&geo_path).unwrap();
    assert!(crs::is_geographic(&g.crs_proj4));
    assert_eq!(g.dx_deg, dscale, "geographic keeps degree scale in dx_deg");
    let expect = dscale * 111_320.0 * g.origin_lat.to_radians().cos();
    assert!(
        (g.dx_meters - expect).abs() < 1e-6,
        "geographic dx_meters {} vs derived {}",
        g.dx_meters,
        expect
    );
}

#[test]
fn overview_cache_round_trip_preserves_crs() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.tif");

    // 2 m/px source < 5 m ⇒ ensure_overview_cache builds TWO overviews
    // (close ≈8 m at factor 4, base ≈32 m at factor 16). 96 is divisible by both.
    let cols = 96;
    let rows = 96;
    let geo_dir = projected_dir(3035);
    write_min_geotiff(
        &src,
        cols,
        rows,
        (2.0, 2.0),
        (4_400_000.0, 2_700_000.0),
        &geo_dir,
        &ramp(cols, rows),
    );

    let cache = ensure_overview_cache(&src, |_, _| {})
        .expect("cache build must not error")
        .expect("a single-IFD sub-5m tile needs a cache");

    // The cache is a multi-IFD overview pyramid with a coarsest level near 32 m.
    let scales = ifd_scales(&cache).unwrap();
    assert!(
        scales.len() >= 2,
        "cache should be multi-IFD, got {scales:?}"
    );
    let coarsest = *scales.last().unwrap();
    assert!(
        (coarsest - 32.0).abs() < 4.0,
        "coarsest overview ≈32 m, got {coarsest}"
    );

    // CRS tags must be copied verbatim so the cache stays self-describing.
    let src_tags = crs::read_raw_crs_tags(&src).unwrap();
    let cache_tags = crs::read_raw_crs_tags(&cache).unwrap();
    assert_eq!(
        src_tags.geo_key_directory, cache_tags.geo_key_directory,
        "GeoKeyDirectory must survive the cache round-trip"
    );
    assert_eq!(src_tags.geo_double_params, cache_tags.geo_double_params);
    assert_eq!(src_tags.geo_ascii_params, cache_tags.geo_ascii_params);
}

#[test]
fn float_max_sentinel_maps_to_nodata() {
    // The BEV ALS DSM declares NoData = +3.4028235e38 via GDAL_NODATA. Before the
    // generalized predicate, those cells parsed as valid 3.4e38 m elevations.
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("dsm.tif");
    let mut data = ramp(8, 8);
    data[0] = 3.402_823_5e38;
    data[27] = 3.402_823_5e38;
    write_min_geotiff_with_nodata(
        &src,
        8,
        8,
        (1.0, 1.0),
        (4_450_000.0, 2_700_000.0),
        &projected_dir(3035),
        &data,
        "3.4028235e+38",
    );

    // extract_window does not infill, so sentinels must surface as exact -9999.
    let win = dem_io::extract_window(&src, (4_450_004.0, 2_699_996.0), 10.0, 0).unwrap();
    assert_eq!((win.cols, win.rows), (8, 8));
    assert_eq!(win.data[0], -9999.0, "float-max sentinel → NODATA");
    assert_eq!(win.data[27], -9999.0);
    assert!(
        win.data.iter().all(|&v| v < 1.0e38),
        "no float-max value may survive into the heightmap"
    );
    assert_eq!(win.data[1], data[1], "valid neighbours pass through");
}

#[test]
fn declared_gdal_nodata_value_maps_to_nodata() {
    // A finite sentinel inside the "plausible elevation" range is only catchable
    // via the GDAL_NODATA tag — neither the NaN nor the ±magnitude checks fire.
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("custom_nodata.tif");
    let mut data = ramp(8, 8);
    data[5] = -500.0;
    data[42] = -500.0;
    write_min_geotiff_with_nodata(
        &src,
        8,
        8,
        (1.0, 1.0),
        (4_450_000.0, 2_700_000.0),
        &projected_dir(3035),
        &data,
        "-500",
    );

    let win = dem_io::extract_window(&src, (4_450_004.0, 2_699_996.0), 10.0, 0).unwrap();
    assert_eq!(win.data[5], -9999.0, "tag-declared sentinel → NODATA");
    assert_eq!(win.data[42], -9999.0);
    assert_eq!(win.data[6], data[6], "valid neighbours pass through");
}

#[test]
fn no_cache_built_for_coarse_single_ifd_source() {
    // A ≥20 m/px source can be served directly by base-tier select_ifd, so
    // ensure_overview_cache must short-circuit to None (no wasted .tmp file).
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("coarse.tif");
    write_min_geotiff(
        &src,
        16,
        16,
        (25.0, 25.0),
        (4_400_000.0, 2_700_000.0),
        &projected_dir(3035),
        &ramp(16, 16),
    );
    let result = ensure_overview_cache(&src, |_, _| {}).unwrap();
    assert!(result.is_none(), "coarse source must not get a cache");
}
