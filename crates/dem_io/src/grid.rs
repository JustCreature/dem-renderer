use std::path::{Path, PathBuf};

use crate::Heightmap;

fn tile_path(tiles_dir: &Path, lat: i32, lon: i32) -> PathBuf {
    let name = format!("Copernicus_DSM_COG_10_N{:02}_00_E{:03}_00_DEM", lat, lon);
    tiles_dir.join(&name).join(format!("{}.tif", name))
}

/// Like `load_grid` but uses an explicit path list instead of directory convention.
/// Loads every path, reads tile position from `origin_lat`/`origin_lon` metadata,
/// finds the NW-most tile, derives the 3×3 centre, and assembles. No filename parsing.
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

    // NW-most tile = highest lat, lowest lon.
    // NW offset in a 3×3 grid is (+1, -1) from centre, so centre = (nw_lat-1, nw_lon+1).
    let nw_lat = tile_map.keys().map(|(la, _)| *la).max().unwrap();
    let nw_lon = tile_map.keys().map(|(_, lo)| *lo).min().unwrap();
    let centre_lat = nw_lat - 1;
    let centre_lon = nw_lon + 1;

    let offsets = [
        [(1, -1), (1, 0), (1, 1)],
        [(0, -1), (0, 0), (0, 1)],
        [(-1, -1), (-1, 0), (-1, 1)],
    ];

    let tiles: [[Option<Heightmap>; 3]; 3] = std::array::from_fn(|row| {
        std::array::from_fn(|col| {
            let (dlat, dlon) = offsets[row][col];
            tile_map.remove(&(centre_lat + dlat, centre_lon + dlon))
        })
    });

    let grid: [[Option<&Heightmap>; 3]; 3] =
        std::array::from_fn(|row| std::array::from_fn(|col| tiles[row][col].as_ref()));

    assemble_grid(&grid)
}

pub fn load_grid<F>(tiles_dir: &Path, centre_lat: i32, centre_lon: i32, loader: F) -> Heightmap
where
    F: Fn(&Path) -> Option<Heightmap>,
{
    let offsets = [
        [(1, -1), (1, 0), (1, 1)],
        [(0, -1), (0, 0), (0, 1)],
        [(-1, -1), (-1, 0), (-1, 1)],
    ];

    let tiles: [[Option<Heightmap>; 3]; 3] = std::array::from_fn(|row| {
        std::array::from_fn(|col| {
            let (dlat, dlon) = offsets[row][col];
            let path = tile_path(tiles_dir, centre_lat + dlat, centre_lon + dlon);
            loader(&path)
        })
    });

    let grid: [[Option<&Heightmap>; 3]; 3] =
        std::array::from_fn(|row| std::array::from_fn(|col| tiles[row][col].as_ref()));

    assemble_grid(&grid)
}

pub fn assemble_grid(grid: &[[Option<&Heightmap>; 3]; 3]) -> Heightmap {
    let nw_tile: &Heightmap =
        grid[0][0].expect("no NW tile provided, NW should always be provided");

    // assemble_grid indexes every sibling tile using nw_tile.cols/rows. That's correct
    // for the only current callers (3×3 Copernicus GLO-30 grids, where all tiles share
    // a 3600×3600 pixel layout) but it's a latent assumption — a mixed-resolution or
    // partial-overview grid would silently produce shifted rows or panic on slice OOB.
    // Catch that misuse before it surfaces as corrupted terrain.
    debug_assert!(
        grid.iter().flatten().flatten().all(|t| t.cols == nw_tile.cols && t.rows == nw_tile.rows),
        "assemble_grid: all tiles must match nw_tile dims ({}×{}), got mismatch — sibling tile sources are not uniform",
        nw_tile.rows, nw_tile.cols,
    );

    let mut assembled_data: Vec<f32> =
        Vec::with_capacity(grid.len() * nw_tile.rows * grid[0].len() * nw_tile.cols);

    for tile_row in 0..3 {
        for pixel_row in 0..nw_tile.rows {
            for tile_col in 0..3 {
                match grid[tile_row][tile_col] {
                    None => assembled_data.extend(std::iter::repeat(0.0f32).take(nw_tile.cols)),
                    Some(hm) => assembled_data.extend_from_slice(
                        &hm.data[pixel_row * nw_tile.cols..(pixel_row + 1) * nw_tile.cols],
                    ),
                }
            }
        }
    }

    Heightmap {
        data: assembled_data,
        rows: nw_tile.rows * grid.len(),
        cols: nw_tile.cols * grid[0].len(),
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
