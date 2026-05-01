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
    }
}
