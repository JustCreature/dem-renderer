use std::path::{Path, PathBuf};

use crate::Heightmap;

fn tile_path(tiles_dir: &Path, lat: i32, lon: i32) -> PathBuf {
    let name = format!("Copernicus_DSM_COG_10_N{:02}_00_E{:03}_00_DEM", lat, lon);
    tiles_dir.join(&name).join(format!("{}.tif", name))
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
    let crs_origin_x = hm.crs_origin_x + col_start as f64 * hm.dx_meters;
    let crs_origin_y = hm.crs_origin_y - row_start as f64 * hm.dy_meters;

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
