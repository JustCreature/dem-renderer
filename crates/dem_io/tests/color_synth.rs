//! Synthetic-fixture tests for the color-window reader.
//!
//! `extract_color_window` and `landcover_histogram` only run end-to-end against
//! the multi-GB BEV mosaics, which CI never has — so the worker decode loop, the
//! land-cover resample and the material mapping showed as uncovered. These build
//! tiny RGB + single-band GeoTIFFs in-process (no JPEG / 4-bit needed: the RGB
//! photometric path passes through, the Gray8 path exercises the byte reader)
//! and assert the decoded window, the packed material codes, and the histogram.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use dem_io::{MATERIAL_HIGH_VEG, MATERIAL_NONE, MATERIAL_WATER};
use tiff::encoder::{TiffEncoder, colortype};
use tiff::tags::Tag;

/// GeoKeyDirectory for a projected CRS by EPSG (GeoKey 3072).
fn projected_dir(epsg: u16) -> Vec<u16> {
    vec![1, 1, 0, 2, 1024, 0, 1, 1, 3072, 0, 1, epsg]
}

/// Write a 3-band RGB byte GeoTIFF (PhotometricInterpretation = RGB, so the
/// reader takes the pass-through branch rather than YCbCr conversion).
fn write_rgb(path: &Path, cols: u32, rows: u32, origin: (f64, f64), rgb: &[u8]) {
    let file = File::create(path).unwrap();
    let mut enc = TiffEncoder::new(BufWriter::new(file)).unwrap();
    let mut img = enc.new_image::<colortype::RGB8>(cols, rows).unwrap();
    img.encoder()
        .write_tag(Tag::Unknown(33550), &[1.0_f64, 1.0, 0.0][..])
        .unwrap();
    img.encoder()
        .write_tag(
            Tag::Unknown(33922),
            &[0.0_f64, 0.0, 0.0, origin.0, origin.1, 0.0][..],
        )
        .unwrap();
    img.encoder()
        .write_tag(Tag::Unknown(34735), projected_dir(3035).as_slice())
        .unwrap();
    img.write_data(rgb).unwrap();
}

/// Write a single-band byte GeoTIFF used as the land-cover raster.
fn write_gray(path: &Path, cols: u32, rows: u32, origin: (f64, f64), g: &[u8]) {
    let file = File::create(path).unwrap();
    let mut enc = TiffEncoder::new(BufWriter::new(file)).unwrap();
    let mut img = enc.new_image::<colortype::Gray8>(cols, rows).unwrap();
    img.encoder()
        .write_tag(Tag::Unknown(33550), &[1.0_f64, 1.0, 0.0][..])
        .unwrap();
    img.encoder()
        .write_tag(
            Tag::Unknown(33922),
            &[0.0_f64, 0.0, 0.0, origin.0, origin.1, 0.0][..],
        )
        .unwrap();
    img.encoder()
        .write_tag(Tag::Unknown(34735), projected_dir(3035).as_slice())
        .unwrap();
    img.write_data(g).unwrap();
}

#[test]
fn color_window_passes_through_rgb_and_packs_material_codes() {
    let dir = tempfile::tempdir().unwrap();
    let (cols, rows) = (64u32, 64u32);
    let origin = (4_400_000.0, 2_700_000.0); // EPSG:3035 top-left (easting, northing)

    // RGB: left half pure red, right half pure blue — distinct enough to assert
    // the pass-through and the column split survive the windowed read.
    let mut rgb = vec![0u8; (cols * rows * 3) as usize];
    for r in 0..rows {
        for c in 0..cols {
            let i = ((r * cols + c) * 3) as usize;
            if c < cols / 2 {
                rgb[i] = 200; // red
            } else {
                rgb[i + 2] = 200; // blue
            }
        }
    }
    let rgb_path = dir.path().join("ortho.tif");
    write_rgb(&rgb_path, cols, rows, origin, &rgb);

    // Land cover on the identical grid: left half water (class 5), right half
    // high vegetation (class 1) → alpha must be WATER on the left, HIGH_VEG on
    // the right, after the nearest-resample onto the ortho grid.
    let mut lc = vec![0u8; (cols * rows) as usize];
    for r in 0..rows {
        for c in 0..cols {
            lc[(r * cols + c) as usize] = if c < cols / 2 { 5 } else { 1 };
        }
    }
    let lc_path = dir.path().join("landcover.tif");
    write_gray(&lc_path, cols, rows, origin, &lc);

    // Window centred on the tile centre, radius 24 px → ~48×48 clipped window.
    let centre = (origin.0 + 32.0, origin.1 - 32.0);
    let win = dem_io::extract_color_window(&rgb_path, Some(&lc_path), centre, 24.0, 0, Some(0))
        .expect("color window");

    let (wc, wr) = (win.georef.cols, win.georef.rows);
    assert!(wc >= 40 && wr >= 40, "expected ~48² window, got {wc}×{wr}");
    assert_eq!(win.rgba.len(), wc * wr * 4);
    assert_eq!(win.georef.dx_meters, 1.0, "1 m/px carried into georef");
    assert!(win.georef.data.is_empty(), "georef is a placement stub");

    // Sample a pixel in the left quarter (red + water) and the right quarter
    // (blue + high veg). Window origin maps to a source column ≥ 8, so quarter
    // offsets stay clear of the centre split.
    let left = ((wr / 2) * wc + wc / 4) * 4;
    let right = ((wr / 2) * wc + (3 * wc) / 4) * 4;
    assert_eq!(win.rgba[left], 200, "left half red channel passes through");
    assert_eq!(win.rgba[left + 2], 0, "left half has no blue");
    assert_eq!(win.rgba[left + 3], MATERIAL_WATER, "left half water material");
    assert_eq!(win.rgba[right + 2], 200, "right half blue channel");
    assert_eq!(win.rgba[right], 0, "right half has no red");
    assert_eq!(
        win.rgba[right + 3],
        MATERIAL_HIGH_VEG,
        "right half high-veg material"
    );
}

#[test]
fn color_window_without_landcover_leaves_material_zero() {
    let dir = tempfile::tempdir().unwrap();
    let (cols, rows) = (32u32, 32u32);
    let origin = (4_400_000.0, 2_700_000.0);
    let rgb = vec![150u8; (cols * rows * 3) as usize];
    let rgb_path = dir.path().join("ortho_only.tif");
    write_rgb(&rgb_path, cols, rows, origin, &rgb);

    let centre = (origin.0 + 16.0, origin.1 - 16.0);
    let win =
        dem_io::extract_color_window(&rgb_path, None, centre, 10.0, 0, None).expect("color window");
    assert!(
        win.rgba.chunks_exact(4).all(|p| p[3] == MATERIAL_NONE),
        "no land cover → every material code stays NONE"
    );
    assert!(
        win.rgba.chunks_exact(4).all(|p| p[0] == 150),
        "flat ortho passes through unchanged"
    );
}

#[test]
fn landcover_histogram_counts_class_values() {
    let dir = tempfile::tempdir().unwrap();
    let (cols, rows) = (40u32, 40u32);
    let origin = (4_400_000.0, 2_700_000.0);
    // Three horizontal bands of classes 5 / 1 / 6.
    let mut g = vec![0u8; (cols * rows) as usize];
    for r in 0..rows {
        let v = if r < 13 { 5 } else if r < 26 { 1 } else { 6 };
        for c in 0..cols {
            g[(r * cols + c) as usize] = v;
        }
    }
    let lc_path = dir.path().join("lc_hist.tif");
    write_gray(&lc_path, cols, rows, origin, &g);

    // Full-tile window (radius covers the whole grid).
    let centre = (origin.0 + 20.0, origin.1 - 20.0);
    let hist = dem_io::landcover_histogram(&lc_path, centre, 100.0, 0).expect("histogram");
    assert!(hist[5] > 0 && hist[1] > 0 && hist[6] > 0, "all three classes present");
    assert_eq!(
        hist.iter().sum::<u64>(),
        (cols * rows) as u64,
        "every pixel counted exactly once"
    );
    assert_eq!(hist[2], 0, "absent class has zero count");
}
