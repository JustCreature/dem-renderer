use std::path::{Path, PathBuf};

use crate::Heightmap;

/// Loads every path, reads tile position from `origin_lat`/`origin_lon` metadata,
/// then assembles an N×M rectangle covering the full bounding box of the supplied
/// tiles. Missing tiles inside the bounding box are filled with 0.0. The caller
/// chooses the grid extent by deciding which tiles to pass in.
pub fn load_grid_from_paths<F>(paths: &[PathBuf], loader: F) -> Heightmap
where
    F: Fn(&Path) -> Option<Heightmap>,
{
    use std::collections::HashMap;

    // Load all tiles and key them by (tile_lat, tile_lon).
    // GLO-30 pixel centres sit ~0.5/3600° inside the integer-degree boundary, so
    // floor(origin_lat) yields the tile's south edge degree label (e.g. 47 for N47).
    let mut tile_map: HashMap<(i32, i32), Heightmap> = paths
        .iter()
        .filter_map(|p| {
            let hm = loader(p)?;
            let tile_lat = hm.origin_lat.floor() as i32;
            let tile_lon = hm.origin_lon.floor() as i32;
            Some(((tile_lat, tile_lon), hm))
        })
        .collect();

    assert!(
        !tile_map.is_empty(),
        "load_grid_from_paths: no tiles loaded"
    );

    // Bounding box over actual loaded tiles. The grid is (max_lat - min_lat + 1)
    // rows × (max_lon - min_lon + 1) cols; any missing cell stays None and
    // assemble_grid fills it with zeros.
    let max_lat = tile_map.keys().map(|(la, _)| *la).max().unwrap();
    let min_lat = tile_map.keys().map(|(la, _)| *la).min().unwrap();
    let min_lon = tile_map.keys().map(|(_, lo)| *lo).min().unwrap();
    let max_lon = tile_map.keys().map(|(_, lo)| *lo).max().unwrap();
    let n_rows = (max_lat - min_lat + 1) as usize;
    let n_cols = (max_lon - min_lon + 1) as usize;

    // Row 0 = northern-most strip (max_lat), col 0 = western-most (min_lon).
    let tiles: Vec<Vec<Option<Heightmap>>> = (0..n_rows)
        .map(|row| {
            let lat = max_lat - row as i32;
            (0..n_cols)
                .map(|col| {
                    let lon = min_lon + col as i32;
                    tile_map.remove(&(lat, lon))
                })
                .collect()
        })
        .collect();

    let grid: Vec<Vec<Option<&Heightmap>>> = tiles
        .iter()
        .map(|row| row.iter().map(|t| t.as_ref()).collect())
        .collect();

    assemble_grid(&grid)
}

/// Assemble an N×M rectangular grid of equally-sized tiles into one heightmap.
/// `grid[0][0]` is the NW corner and must be present (defines tile dims and origin).
/// Any other cell may be `None` and is filled with zeros.
pub fn assemble_grid(grid: &[Vec<Option<&Heightmap>>]) -> Heightmap {
    assert!(!grid.is_empty(), "assemble_grid: empty grid");
    let n_rows = grid.len();
    let n_cols = grid[0].len();
    assert!(n_cols > 0, "assemble_grid: empty row");
    assert!(
        grid.iter().all(|row| row.len() == n_cols),
        "assemble_grid: ragged grid (rows must all have the same length)"
    );

    let nw_tile: &Heightmap =
        grid[0][0].expect("no NW tile provided, NW should always be provided");

    // assemble_grid indexes every sibling tile using nw_tile.cols/rows. That's correct
    // for the only current callers (Copernicus GLO-30 grids, where all tiles share
    // a 3600×3600 pixel layout) but it's a latent assumption — a mixed-resolution or
    // partial-overview grid would silently produce shifted rows or panic on slice OOB.
    // Catch that misuse before it surfaces as corrupted terrain.
    debug_assert!(
        grid.iter()
            .flatten()
            .flatten()
            .all(|t| t.cols == nw_tile.cols && t.rows == nw_tile.rows),
        "assemble_grid: all tiles must match nw_tile dims ({}×{}), got mismatch — sibling tile sources are not uniform",
        nw_tile.rows,
        nw_tile.cols,
    );

    let mut assembled_data: Vec<f32> =
        Vec::with_capacity(n_rows * nw_tile.rows * n_cols * nw_tile.cols);

    for grid_row in grid {
        for pixel_row in 0..nw_tile.rows {
            for cell in grid_row {
                match cell {
                    None => assembled_data.extend(std::iter::repeat_n(0.0f32, nw_tile.cols)),
                    Some(hm) => assembled_data.extend_from_slice(
                        &hm.data[pixel_row * nw_tile.cols..(pixel_row + 1) * nw_tile.cols],
                    ),
                }
            }
        }
    }

    Heightmap {
        data: assembled_data,
        rows: nw_tile.rows * n_rows,
        cols: nw_tile.cols * n_cols,
        nodata: nw_tile.nodata,
        origin_lat: nw_tile.origin_lat,
        origin_lon: nw_tile.origin_lon,
        dx_deg: nw_tile.dx_deg,
        dy_deg: nw_tile.dy_deg,
        dx_meters: nw_tile.dx_meters,
        dy_meters: nw_tile.dy_meters,
        crs_origin_x: nw_tile.crs_origin_x,
        crs_origin_y: nw_tile.crs_origin_y,
        crs_epsg: nw_tile.crs_epsg,
        crs_proj4: nw_tile.crs_proj4.clone(),
    }
}

/// Merge multiple `Heightmap` windows (same CRS and resolution) into one output grid
/// covering [centre_e±radius_m) × [centre_n±radius_m). Pixels from each window are placed
/// by computing pixel offsets from the output origin using the window's `crs_origin_x/y`.
/// NODATA cells (-9999 or NaN) in a source window are skipped, so any window can partially
/// fill the output without overwriting valid data from another window.
pub fn stitch_windows(
    windows: Vec<Heightmap>,
    centre_e: f64,
    centre_n: f64,
    radius_m: f64,
) -> Heightmap {
    let out_cols = (2.0 * radius_m) as usize;
    let out_rows = (2.0 * radius_m) as usize;
    let out_e0 = centre_e - radius_m; // left edge easting
    let out_n1 = centre_n + radius_m; // top edge northing
    const NODATA: f32 = -9999.0;
    let mut data = vec![NODATA; out_cols * out_rows];

    for win in &windows {
        let col_offset = ((win.crs_origin_x - out_e0) / win.dx_meters).round() as isize;
        let row_offset = ((out_n1 - win.crs_origin_y) / win.dy_meters).round() as isize;
        for wr in 0..win.rows {
            let or_ = row_offset + wr as isize;
            if or_ < 0 || or_ >= out_rows as isize {
                continue;
            }
            for wc in 0..win.cols {
                let oc = col_offset + wc as isize;
                if oc < 0 || oc >= out_cols as isize {
                    continue;
                }
                let v = win.data[wr * win.cols + wc];
                if v != NODATA && !v.is_nan() {
                    data[or_ as usize * out_cols + oc as usize] = v;
                }
            }
        }
    }

    let first = &windows[0];
    Heightmap {
        data,
        rows: out_rows,
        cols: out_cols,
        nodata: NODATA,
        crs_origin_x: out_e0,
        crs_origin_y: out_n1,
        dx_meters: first.dx_meters,
        dy_meters: first.dy_meters,
        crs_epsg: first.crs_epsg,
        crs_proj4: first.crs_proj4.clone(),
        origin_lat: first.origin_lat,
        origin_lon: first.origin_lon,
        dx_deg: first.dx_deg,
        dy_deg: first.dy_deg,
    }
}

/// Like `stitch_windows` but for WGS84 geographic tiles where `crs_origin_x` = lon,
/// `crs_origin_y` = lat, and `dx_meters`/`dy_meters` store deg/px (as returned by
/// `extract_window` for geographic tiles).  After stitching the output gets its
/// `dx_meters`/`dy_meters` fixed up to actual m/px at `centre_lat`.
pub fn stitch_windows_geographic(
    windows: Vec<Heightmap>,
    centre_lon: f64,
    centre_lat: f64,
    radius_lon_deg: f64,
    radius_lat_deg: f64,
) -> Heightmap {
    let first = &windows[0];
    let deg_per_px_x = first.dx_meters; // dx_meters stores deg/px for geographic extract_window
    let deg_per_px_y = first.dy_meters;

    let out_lon0 = centre_lon - radius_lon_deg;
    let out_lat1 = centre_lat + radius_lat_deg;
    let out_cols = ((2.0 * radius_lon_deg) / deg_per_px_x).round() as usize;
    let out_rows = ((2.0 * radius_lat_deg) / deg_per_px_y).round() as usize;

    const NODATA: f32 = -9999.0;
    let mut data = vec![NODATA; out_cols * out_rows];

    for win in &windows {
        let col_offset = ((win.crs_origin_x - out_lon0) / win.dx_meters).round() as isize;
        let row_offset = ((out_lat1 - win.crs_origin_y) / win.dy_meters).round() as isize;
        for wr in 0..win.rows {
            let or_ = row_offset + wr as isize;
            if or_ < 0 || or_ >= out_rows as isize {
                continue;
            }
            for wc in 0..win.cols {
                let oc = col_offset + wc as isize;
                if oc < 0 || oc >= out_cols as isize {
                    continue;
                }
                let v = win.data[wr * win.cols + wc];
                if v != NODATA && !v.is_nan() {
                    data[or_ as usize * out_cols + oc as usize] = v;
                }
            }
        }
    }

    // Fix up dx_meters/dy_meters to actual m/px.
    // Use the NW corner latitude (out_lat1 = crs_origin_y) so that dx_meters is consistent
    // with how cam_pos.x is built: latlon_to_tile_metres uses cos(crs_origin_y), so
    // cam.dx_meters must also use cos(crs_origin_y) for the shader column index to be correct.
    let actual_dx_m = deg_per_px_x * 111_320.0 * out_lat1.to_radians().cos();
    let actual_dy_m = deg_per_px_y * 111_320.0;

    Heightmap {
        data,
        rows: out_rows,
        cols: out_cols,
        nodata: NODATA,
        crs_origin_x: out_lon0,
        crs_origin_y: out_lat1,
        dx_meters: actual_dx_m,
        dy_meters: actual_dy_m,
        crs_epsg: first.crs_epsg,
        crs_proj4: first.crs_proj4.clone(),
        origin_lat: centre_lat,
        origin_lon: centre_lon,
        dx_deg: deg_per_px_x,
        dy_deg: deg_per_px_y,
    }
}

pub fn crop(
    hm: &Heightmap,
    row_start: usize,
    col_start: usize,
    rows: usize,
    cols: usize,
) -> Heightmap {
    let mut data: Vec<f32> = Vec::with_capacity(rows * cols);

    for r in 0..rows {
        let row_offset = (row_start + r) * hm.cols + col_start;
        data.extend_from_slice(&hm.data[row_offset..row_offset + cols]);
    }

    let origin_lat = hm.origin_lat - row_start as f64 * hm.dy_deg.abs();
    let origin_lon = hm.origin_lon + col_start as f64 * hm.dx_deg;
    // For geographic tiles crs_origin_x/y are in degrees — advance using dx_deg.
    // For projected tiles they are in metres — advance using dx_meters.
    let (crs_origin_x, crs_origin_y) = if hm.dx_deg != 0.0 {
        (
            hm.crs_origin_x + col_start as f64 * hm.dx_deg,
            hm.crs_origin_y - row_start as f64 * hm.dy_deg.abs(),
        )
    } else {
        (
            hm.crs_origin_x + col_start as f64 * hm.dx_meters,
            hm.crs_origin_y - row_start as f64 * hm.dy_meters,
        )
    };

    Heightmap {
        data,
        rows,
        cols,
        nodata: hm.nodata,
        origin_lat,
        origin_lon,
        dx_deg: hm.dx_deg,
        dy_deg: hm.dy_deg,
        dx_meters: hm.dx_meters,
        dy_meters: hm.dy_meters,
        crs_origin_x,
        crs_origin_y,
        crs_epsg: hm.crs_epsg,
        crs_proj4: hm.crs_proj4.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(rows: usize, cols: usize, data: Vec<f32>) -> Heightmap {
        assert_eq!(data.len(), rows * cols);
        Heightmap {
            data,
            rows,
            cols,
            nodata: -9999.0,
            origin_lat: 0.0,
            origin_lon: 0.0,
            dx_deg: 0.0,
            dy_deg: 0.0,
            dx_meters: 1.0,
            dy_meters: 1.0,
            crs_origin_x: 0.0,
            crs_origin_y: 0.0,
            crs_epsg: 0,
            crs_proj4: String::new(),
        }
    }

    // ----- assemble_grid ---------------------------------------------------

    #[test]
    fn assemble_grid_interleaves_tile_rows_correctly() {
        // 2×2 grid of 2×2 tiles → 4×4 output. The crucial property is that within a
        // strip of tile-rows, pixel rows from adjacent tile-columns are interleaved.
        let nw = mk(2, 2, vec![1.0, 2.0, 3.0, 4.0]);
        let ne = mk(2, 2, vec![5.0, 6.0, 7.0, 8.0]);
        let sw = mk(2, 2, vec![9.0, 10.0, 11.0, 12.0]);
        let se = mk(2, 2, vec![13.0, 14.0, 15.0, 16.0]);
        let grid = vec![vec![Some(&nw), Some(&ne)], vec![Some(&sw), Some(&se)]];
        let out = assemble_grid(&grid);
        assert_eq!(out.rows, 4);
        assert_eq!(out.cols, 4);
        #[rustfmt::skip]
        let expected = vec![
            1.0, 2.0, 5.0, 6.0,
            3.0, 4.0, 7.0, 8.0,
            9.0, 10.0, 13.0, 14.0,
            11.0, 12.0, 15.0, 16.0,
        ];
        assert_eq!(out.data, expected);
        // Metadata inherited from the NW tile.
        assert_eq!(out.nodata, nw.nodata);
        assert_eq!(out.crs_epsg, nw.crs_epsg);
    }

    #[test]
    fn assemble_grid_fills_none_cells_with_zeros() {
        let nw = mk(2, 2, vec![1.0, 2.0, 3.0, 4.0]);
        let grid = vec![vec![Some(&nw), None]];
        let out = assemble_grid(&grid);
        assert_eq!((out.rows, out.cols), (2, 4));
        #[rustfmt::skip]
        let expected = vec![
            1.0, 2.0, 0.0, 0.0,
            3.0, 4.0, 0.0, 0.0,
        ];
        assert_eq!(out.data, expected);
    }

    #[test]
    #[should_panic(expected = "empty grid")]
    fn assemble_grid_empty_grid_panics() {
        let grid: Vec<Vec<Option<&Heightmap>>> = vec![];
        assemble_grid(&grid);
    }

    #[test]
    #[should_panic(expected = "empty row")]
    fn assemble_grid_empty_row_panics() {
        let grid: Vec<Vec<Option<&Heightmap>>> = vec![vec![]];
        assemble_grid(&grid);
    }

    #[test]
    #[should_panic(expected = "NW tile")]
    fn assemble_grid_missing_nw_panics() {
        let grid: Vec<Vec<Option<&Heightmap>>> = vec![vec![None]];
        assemble_grid(&grid);
    }

    #[test]
    #[should_panic(expected = "ragged grid")]
    fn assemble_grid_ragged_panics() {
        let a = mk(1, 1, vec![1.0]);
        let grid = vec![vec![Some(&a)], vec![Some(&a), Some(&a)]];
        assemble_grid(&grid);
    }

    #[test]
    #[should_panic] // debug_assert fires under `cargo test` (debug build)
    fn assemble_grid_mismatched_sibling_dims_panics() {
        let nw = mk(2, 2, vec![1.0; 4]);
        let odd = mk(1, 1, vec![9.0]);
        let grid = vec![vec![Some(&nw), Some(&odd)]];
        assemble_grid(&grid);
    }

    // ----- crop ------------------------------------------------------------

    #[test]
    fn crop_full_extent_is_identity_data() {
        let hm = mk(4, 4, (0..16).map(|i| i as f32).collect());
        let out = crop(&hm, 0, 0, 4, 4);
        assert_eq!(out.data, hm.data);
    }

    #[test]
    fn crop_subwindow_extracts_correct_block() {
        let hm = mk(4, 4, (0..16).map(|i| i as f32).collect());
        let out = crop(&hm, 1, 1, 2, 2);
        assert_eq!(out.data, vec![5.0, 6.0, 9.0, 10.0]);
    }

    #[test]
    fn crop_projected_advances_crs_origin_by_meters() {
        // dx_deg == 0 → projected branch: crs_origin advances by dx_meters/dy_meters.
        let mut hm = mk(4, 4, vec![0.0; 16]);
        hm.dx_meters = 5.0;
        hm.dy_meters = 5.0;
        hm.crs_origin_x = 1000.0;
        hm.crs_origin_y = 2000.0;
        let out = crop(&hm, 1, 1, 2, 2);
        assert_eq!(out.crs_origin_x, 1005.0);
        assert_eq!(out.crs_origin_y, 1995.0);
    }

    #[test]
    fn crop_geographic_advances_crs_origin_by_degrees() {
        // dx_deg != 0 → geographic branch: crs_origin advances by dx_deg/dy_deg.
        let mut hm = mk(4, 4, vec![0.0; 16]);
        hm.dx_deg = 0.1;
        hm.dy_deg = 0.1;
        hm.crs_origin_x = 10.0;
        hm.crs_origin_y = 47.0;
        hm.origin_lon = 10.0;
        hm.origin_lat = 47.0;
        let out = crop(&hm, 2, 3, 1, 1);
        assert!((out.crs_origin_x - 10.3).abs() < 1e-9);
        assert!((out.crs_origin_y - 46.8).abs() < 1e-9);
        assert!((out.origin_lon - 10.3).abs() < 1e-9);
        assert!((out.origin_lat - 46.8).abs() < 1e-9);
    }

    #[test]
    fn crop_zero_rows_yields_empty() {
        let hm = mk(4, 4, vec![1.0; 16]);
        let out = crop(&hm, 0, 0, 0, 4);
        assert!(out.data.is_empty());
        assert_eq!(out.rows, 0);
    }

    // ----- stitch_windows --------------------------------------------------

    fn placed(centre_e: f64, centre_n: f64, radius: f64, data: Vec<f32>) -> Heightmap {
        // 2×2 window placed exactly at the output's NW corner.
        let mut w = mk(2, 2, data);
        w.crs_origin_x = centre_e - radius;
        w.crs_origin_y = centre_n + radius;
        w
    }

    #[test]
    fn stitch_windows_single_window_fills_output() {
        let w = placed(10.0, 20.0, 1.0, vec![1.0, 2.0, 3.0, 4.0]);
        let out = stitch_windows(vec![w], 10.0, 20.0, 1.0);
        assert_eq!((out.rows, out.cols), (2, 2));
        assert_eq!(out.data, vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn stitch_windows_skips_nodata_so_windows_merge() {
        // Two complementary windows; each carries valid data only on one diagonal,
        // NODATA on the other. Stitched result is order-independent and hole-free.
        let n = -9999.0;
        let a = placed(10.0, 20.0, 1.0, vec![7.0, n, n, 7.0]);
        let b = placed(10.0, 20.0, 1.0, vec![n, 9.0, 9.0, n]);
        let out = stitch_windows(vec![a, b], 10.0, 20.0, 1.0);
        assert_eq!(out.data, vec![7.0, 9.0, 9.0, 7.0]);
    }

    #[test]
    fn stitch_windows_skips_nan() {
        let a = placed(10.0, 20.0, 1.0, vec![f32::NAN, 2.0, 3.0, 4.0]);
        let out = stitch_windows(vec![a], 10.0, 20.0, 1.0);
        // NaN source pixel is skipped → that output cell keeps the NODATA fill.
        assert_eq!(out.data[0], -9999.0);
        assert_eq!(out.data[1], 2.0);
    }

    #[test]
    fn stitch_windows_clips_window_partly_out_of_bounds() {
        // Shift the window one column left so its col 0 maps to output col -1
        // (skipped) and its col 1 maps to output col 0. Must not panic.
        let mut w = placed(10.0, 20.0, 1.0, vec![1.0, 2.0, 3.0, 4.0]);
        w.crs_origin_x -= 1.0; // col_offset becomes -1
        let out = stitch_windows(vec![w], 10.0, 20.0, 1.0);
        assert_eq!(out.data[0], 2.0, "window col 1 lands in output col 0");
    }

    // ----- stitch_windows_geographic --------------------------------------

    #[test]
    fn stitch_geographic_fixes_dx_meters_with_cos_lat() {
        let deg_per_px = 0.001;
        let mut w = mk(20, 20, vec![5.0; 400]);
        // For geographic windows dx_meters/dy_meters store deg/px.
        w.dx_meters = deg_per_px;
        w.dy_meters = deg_per_px;
        w.crs_origin_x = 11.0 - 0.01; // out_lon0
        w.crs_origin_y = 47.0 + 0.01; // out_lat1
        w.crs_proj4 = "+proj=longlat +datum=WGS84 +no_defs".to_string();
        let out = stitch_windows_geographic(vec![w], 11.0, 47.0, 0.01, 0.01);
        assert_eq!((out.rows, out.cols), (20, 20));
        let out_lat1 = 47.01_f64;
        let expect_dx = deg_per_px * 111_320.0 * out_lat1.to_radians().cos();
        assert!(
            (out.dx_meters - expect_dx).abs() < 1e-6,
            "dx_meters {} vs expected {}",
            out.dx_meters,
            expect_dx
        );
        assert!((out.dy_meters - deg_per_px * 111_320.0).abs() < 1e-6);
    }

    // ----- load_grid_from_paths -------------------------------------------

    /// Loader that derives a 1×1 tile from a "lat_lon" filename, value = lon.
    /// Returns None for the sentinel "skip".
    fn fake_loader(p: &Path) -> Option<Heightmap> {
        let name = p.file_name()?.to_string_lossy().to_string();
        if name == "skip" {
            return None;
        }
        let (lat, lon) = name.split_once('_')?;
        let lat: f64 = lat.parse().ok()?;
        let lon: f64 = lon.parse().ok()?;
        let mut hm = mk(1, 1, vec![lon as f32]);
        hm.origin_lat = lat;
        hm.origin_lon = lon;
        Some(hm)
    }

    #[test]
    fn load_grid_assembles_adjacent_tiles_left_to_right() {
        let paths = vec![PathBuf::from("47.0_11.0"), PathBuf::from("47.0_12.0")];
        let out = load_grid_from_paths(&paths, fake_loader);
        assert_eq!((out.rows, out.cols), (1, 2));
        assert_eq!(out.data, vec![11.0, 12.0]);
    }

    #[test]
    fn load_grid_keys_tiles_by_floor_of_origin() {
        // origin just inside the north/east boundary still keys to the integer degree.
        let paths = vec![PathBuf::from("47.9996_11.9996")];
        let out = load_grid_from_paths(&paths, fake_loader);
        assert_eq!((out.rows, out.cols), (1, 1));
        assert_eq!(out.data, vec![11.9996_f32]);
    }

    #[test]
    fn load_grid_fills_interior_gap_with_zeros() {
        // lon 11 and 13 present, 12 missing → 1×3 grid with a zero column in the gap.
        let paths = vec![PathBuf::from("47.0_11.0"), PathBuf::from("47.0_13.0")];
        let out = load_grid_from_paths(&paths, fake_loader);
        assert_eq!((out.rows, out.cols), (1, 3));
        assert_eq!(out.data, vec![11.0, 0.0, 13.0]);
    }

    #[test]
    fn load_grid_filters_paths_the_loader_rejects() {
        let paths = vec![
            PathBuf::from("47.0_11.0"),
            PathBuf::from("skip"),
            PathBuf::from("47.0_12.0"),
        ];
        let out = load_grid_from_paths(&paths, fake_loader);
        assert_eq!(out.data, vec![11.0, 12.0]);
    }

    #[test]
    #[should_panic(expected = "no tiles loaded")]
    fn load_grid_all_rejected_panics() {
        let paths = vec![PathBuf::from("skip"), PathBuf::from("skip")];
        load_grid_from_paths(&paths, fake_loader);
    }
}
