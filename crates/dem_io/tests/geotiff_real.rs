//! Integration tests for the GeoTIFF reader against real, gdal-cut fixtures.
//!
//! These exercise the format/CRS resolution paths that only have meaning against
//! real tag soup: the three-path CRS discovery, geographic vs projected scale
//! derivation, the no-overview (single IFD) case, and the datum-shift injection.

mod common;

use common::{FIXTURES, fixture_path};
use dem_io::{
    crs, extract_window, get_tile_epsg, ifd_scales, parse_geotiff_auto, tile_bounds_wgs84,
    tile_centre_crs,
};

/// Non-sentinel, finite elevation values from a parsed window.
fn valid_elevations(data: &[f32]) -> Vec<f32> {
    data.iter()
        .copied()
        .filter(|&v| v.is_finite() && v > -9000.0)
        .collect()
}

#[test]
fn parse_reports_expected_dims_crs_and_geographic_flag() {
    for fx in FIXTURES {
        let path = fixture_path(fx.file);
        let hm = parse_geotiff_auto(&path).unwrap_or_else(|e| panic!("{}: {e}", fx.file));

        assert_eq!(hm.rows, 512, "{}: rows", fx.file);
        assert_eq!(hm.cols, 512, "{}: cols", fx.file);
        assert_eq!(hm.crs_epsg, fx.epsg, "{}: crs_epsg", fx.file);
        assert_eq!(
            crs::is_geographic(&hm.crs_proj4),
            fx.geographic,
            "{}: is_geographic ({})",
            fx.file,
            hm.crs_proj4
        );

        // Every parsed value must be finite (NaN/inf sentinels mapped away).
        assert!(
            hm.data.iter().all(|v| v.is_finite()),
            "{}: non-finite value leaked into parsed data",
            fx.file
        );
        let valid = valid_elevations(&hm.data);
        assert!(!valid.is_empty(), "{}: no valid elevations", fx.file);
        let max = valid.iter().cloned().fold(f32::MIN, f32::max);
        assert!(
            (-500.0..9000.0).contains(&max),
            "{}: implausible max elevation {max}",
            fx.file
        );
    }
}

#[test]
fn get_tile_epsg_matches_table() {
    for fx in FIXTURES {
        let epsg =
            get_tile_epsg(&fixture_path(fx.file)).unwrap_or_else(|e| panic!("{}: {e}", fx.file));
        assert_eq!(epsg, fx.epsg, "{}", fx.file);
    }
}

#[test]
fn tile_bounds_land_in_expected_region() {
    for fx in FIXTURES {
        let (lat_min, lat_max, lon_min, lon_max) = tile_bounds_wgs84(&fixture_path(fx.file))
            .unwrap_or_else(|e| panic!("{}: {e}", fx.file));

        assert!(lat_min < lat_max, "{}: lat not ordered", fx.file);
        assert!(lon_min < lon_max, "{}: lon not ordered", fx.file);

        let clat = 0.5 * (lat_min + lat_max);
        let clon = 0.5 * (lon_min + lon_max);
        assert!(
            (fx.lat.0..=fx.lat.1).contains(&clat),
            "{}: centre lat {clat} outside {:?}",
            fx.file,
            fx.lat
        );
        assert!(
            (fx.lon.0..=fx.lon.1).contains(&clon),
            "{}: centre lon {clon} outside {:?}",
            fx.file,
            fx.lon
        );
    }
}

#[test]
fn ifd_zero_scale_matches_pixel_size() {
    for fx in FIXTURES {
        let scales =
            ifd_scales(&fixture_path(fx.file)).unwrap_or_else(|e| panic!("{}: {e}", fx.file));
        let rel = ((scales[0] - fx.px_scale) / fx.px_scale).abs();
        assert!(
            rel < 1e-3,
            "{}: ifd_scales[0] {} vs expected {}",
            fx.file,
            scales[0],
            fx.px_scale
        );
    }
}

#[test]
fn extract_window_returns_plausible_terrain() {
    for fx in FIXTURES {
        let path = fixture_path(fx.file);
        let centre = tile_centre_crs(&path).unwrap_or_else(|e| panic!("{}: {e}", fx.file));
        // radius in the same units as the tile's pixel scale (m for projected,
        // deg for geographic) → ~±50 px window around the centre.
        let radius = fx.px_scale * 50.0;
        let win = extract_window(&path, centre, radius, 0)
            .unwrap_or_else(|e| panic!("{}: extract_window {e}", fx.file));

        assert!(win.rows > 0 && win.cols > 0, "{}: empty window", fx.file);
        let valid = valid_elevations(&win.data);
        assert!(!valid.is_empty(), "{}: window all nodata", fx.file);
        let max = valid.iter().cloned().fold(f32::MIN, f32::max);
        assert!(
            (-500.0..9000.0).contains(&max),
            "{}: implausible window max {max}",
            fx.file
        );
    }
}

// ----- fixture-specific edge cases ----------------------------------------

#[test]
fn newzealand_tile_has_a_single_ifd() {
    // The user-called-out case: BW20 has no internal overviews. ifd_scales must
    // report exactly one level, IFD-0 extraction works, and seeking IFD-1 errors.
    let path = fixture_path("newzealand_1m_nztm_no_ifd.tif");
    let scales = ifd_scales(&path).unwrap();
    assert_eq!(scales.len(), 1, "expected a single IFD, got {scales:?}");

    let centre = tile_centre_crs(&path).unwrap();
    assert!(
        extract_window(&path, centre, 50.0, 0).is_ok(),
        "IFD-0 must read"
    );
    assert!(
        extract_window(&path, centre, 50.0, 1).is_err(),
        "there is no IFD-1 to seek to"
    );
}

#[test]
fn everest_resolves_through_inline_geokey_path() {
    // HMA's projection is user-defined Albers encoded inline (3072 = 32767). The
    // only way to get `+proj=aea` is discovery path 2 (inline GeoKey synthesis);
    // WKT and EPSG lookup cannot produce it. crs_epsg falls back to the geographic
    // base (4326) because there is no projected EPSG.
    let path = fixture_path("everest_hma_8m_albers_inline.tif");
    let p4 = crs::tile_proj4(&path).unwrap();
    assert!(p4.contains("+proj=aea"), "not Albers: {p4}");
    assert!(
        p4.contains("+lat_1=25") && p4.contains("+lat_2=47"),
        "params lost: {p4}"
    );

    let hm = parse_geotiff_auto(&path).unwrap();
    assert_eq!(hm.crs_epsg, 4326, "inline CRS → geographic base epsg");
    assert!(!crs::is_geographic(&hm.crs_proj4), "Albers is projected");
}

#[test]
fn austria_injects_mgi_datum_shift() {
    // Without the built-in epsg_towgs84 override the Austrian grid sits ~600 m off
    // the Copernicus base. The resolved proj4 must carry the MGI 7-parameter shift.
    let p4 = crs::tile_proj4(&fixture_path("austria_dgm_5m_lambert.tif")).unwrap();
    assert!(
        p4.contains("+towgs84=577.326,90.129,463.919"),
        "MGI Helmert shift missing: {p4}"
    );
}

#[test]
fn copernicus_geographic_scale_derivation() {
    // Geographic tile: dx_deg is the raw degree scale, dx_meters is derived as
    // dx_deg * 111320 * cos(lat).
    let hm = parse_geotiff_auto(&fixture_path("copernicus_n47e011_30m_wgs84.tif")).unwrap();
    assert!(crs::is_geographic(&hm.crs_proj4));
    assert!(
        hm.dx_deg != 0.0,
        "geographic tile must keep a non-zero dx_deg"
    );
    let expect = hm.dx_deg * 111_320.0 * hm.origin_lat.to_radians().cos();
    assert!(
        (hm.dx_meters - expect).abs() < 1e-3,
        "dx_meters {} vs derived {}",
        hm.dx_meters,
        expect
    );
}

#[test]
fn oahu_extreme_nodata_sentinel_maps_cleanly() {
    // NoData is −3.4e38 (< −1000) — it must be mapped to the sentinel and never
    // leak as a finite garbage value or NaN/inf into the parsed heightmap.
    let hm = parse_geotiff_auto(&fixture_path("oahu_diamondhead_1m_utm4n.tif")).unwrap();
    assert!(
        hm.data.iter().all(|v| v.is_finite() && *v > -1.0e30),
        "extreme nodata leaked into parsed data"
    );
}
