//! Tests gated on the multi-GB local Tirol tiles (DSM / land cover / RGB ortho).
//! These files are not committed (tiles/ is gitignored); each test skips with a
//! note when its input is absent so CI and other machines stay green.

use std::path::PathBuf;

fn repo_tile(rel: &str) -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tiles")
        .join(rel);
    if p.exists() {
        Some(p)
    } else {
        eprintln!("skipping — {rel} not present");
        None
    }
}

#[test]
fn rgb_ortho_overview_levels_skip_mask_ifds() {
    let Some(path) = repo_tile("color/2019470_Mosaik_RGB.tif") else {
        return;
    };
    let levels = dem_io::ifd_overview_levels(&path).expect("walk IFDs");

    // gdalinfo: full res + 12 overviews = 13 image IFDs; the per-dataset mask
    // IFDs interleaved in the chain must not appear.
    assert_eq!(levels.len(), 13, "13 image levels, masks skipped: {levels:?}");
    assert_eq!(levels[0], (0, 0.2), "IFD 0 is the 0.2 m/px full resolution");
    for pair in levels.windows(2) {
        let ratio = pair[1].1 / pair[0].1;
        assert!(
            (ratio - 2.0).abs() < 0.05,
            "overview scales must roughly double: {levels:?}"
        );
    }
}

#[test]
fn landcover_overview_levels_have_no_masks_to_skip() {
    let Some(path) = repo_tile("color/2022470_Mosaik_LC.tif") else {
        return;
    };
    let levels = dem_io::ifd_overview_levels(&path).expect("walk IFDs");
    // gdalinfo: full res + 10 overviews, no mask bands.
    assert_eq!(levels.len(), 11, "11 image levels: {levels:?}");
    assert_eq!(levels[0], (0, 0.2));
    // Without masks the IFD indices must be contiguous.
    for (i, &(ifd, _)) in levels.iter().enumerate() {
        assert_eq!(ifd, i);
    }
}

#[test]
fn color_window_decodes_albedo_and_water_material() {
    let Some(rgb) = repo_tile("color/2019470_Mosaik_RGB.tif") else {
        return;
    };
    let Some(lc) = repo_tile("color/2022470_Mosaik_LC.tif") else {
        return;
    };
    // Achensee centre: 100% water in the land cover, blue-ish ortho pixels.
    let proj4 = dem_io::crs::tile_proj4(&rgb).expect("CRS");
    let centre = dem_io::crs::from_wgs84(47.4450, 11.7080, &proj4).expect("project");
    // ifd 4 = 1.6 m/px in the RGB pyramid (mask IFDs skipped), lc ifd 3 matches.
    let win = dem_io::extract_color_window(&rgb, Some(&lc), centre, 250.0, 4, Some(3))
        .expect("color window");

    let n = win.georef.rows * win.georef.cols;
    assert_eq!(win.rgba.len(), n * 4);
    assert!(n > 90_000, "expected ≈312² window, got {n}");

    let water_frac = win
        .rgba
        .chunks_exact(4)
        .filter(|p| p[3] == dem_io::MATERIAL_WATER)
        .count() as f64
        / n as f64;
    assert!(
        water_frac > 0.95,
        "lake centre must be nearly all water material, got {water_frac:.2}"
    );

    // The YCbCr→RGB path must produce chroma, not grayscale: over a lake the
    // blue channel average exceeds red by a clear margin.
    let (mut r_sum, mut b_sum) = (0u64, 0u64);
    for p in win.rgba.chunks_exact(4) {
        r_sum += p[0] as u64;
        b_sum += p[2] as u64;
    }
    assert!(
        b_sum > r_sum + (n as u64 * 5),
        "lake should be blue-tinted: avg r={} b={}",
        r_sum / n as u64,
        b_sum / n as u64
    );
}

#[test]
fn dsm_window_maps_float_max_nodata() {
    let Some(path) = repo_tile("big_size/ALS_DSM_CRS3035RES50000mN2650000E4450000.tif") else {
        return;
    };
    // Window at the DSM's western edge (EPSG:3035) — guaranteed to include both
    // valid surface pixels and +3.4e38 NoData from outside the ALS footprint.
    let win = dem_io::extract_window(&path, (4_450_200.0, 2_663_000.0), 400.0, 0).expect("window");
    assert!(
        win.data.iter().all(|&v| v < 1.0e38),
        "no float-max sentinel may survive"
    );
    let valid: Vec<f32> = win.data.iter().copied().filter(|&v| v > -9000.0).collect();
    assert!(!valid.is_empty(), "window must contain valid DSM surface");
    let max = valid.iter().cloned().fold(f32::MIN, f32::max);
    assert!(
        (400.0..4000.0).contains(&max),
        "plausible Tirol surface height, got {max}"
    );
}
