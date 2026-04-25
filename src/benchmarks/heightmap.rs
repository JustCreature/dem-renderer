use crate::utils::*;

use dem_io::{Heightmap, TiledHeightmap};

pub(crate) fn bench_neighbours_rowmajor(hm: &Heightmap) {
    let (ticks, _) = profiling::timed("row_major", || {
        let mut sum: i64 = 0i64;
        for r in 1..hm.rows - 1 {
            for c in 1..hm.cols - 1 {
                sum += hm.data[(r - 1) * hm.cols + c] as i64;
                sum += hm.data[(r + 1) * hm.cols + c] as i64;
                sum += hm.data[r * hm.cols + (c - 1)] as i64;
                sum += hm.data[r * hm.cols + (c + 1)] as i64;
            }
        }
        std::hint::black_box(sum);
    });
    let gb_per_second: f64 = count_gb_per_sec(ticks, Some(4 * 2 * (hm.rows - 2) * (hm.cols - 2)));
    println!("row_major: {:.1} GB/s", gb_per_second);
}

pub(crate) fn bench_neighbours_tiled(hm: &TiledHeightmap) {
    let (ticks, _) = profiling::timed("row_major", || {
        let mut sum: i64 = 0i64;
        for r in 1..hm.rows - 1 {
            for c in 1..hm.cols - 1 {
                sum += hm.get(r - 1, c) as i64;
                sum += hm.get(r + 1, c) as i64;
                sum += hm.get(r, c - 1) as i64;
                sum += hm.get(r, c + 1) as i64;
            }
        }
        std::hint::black_box(sum);
    });
    let gb_per_second: f64 = count_gb_per_sec(ticks, Some(4 * 2 * (hm.rows - 2) * (hm.cols - 2)));
    println!("tiled: {:.1} GB/s", gb_per_second);
}

pub(crate) fn bench_neighbours_tiled_walk_tiles_order(hm: &TiledHeightmap) {
    let (ticks, _) = profiling::timed("row_major", || {
        let mut sum: i64 = 0i64;
        for tr in 0..hm.tile_rows {
            for tc in 0..hm.tile_cols {
                for r in 0..hm.tile_size {
                    for c in 0..hm.tile_size {
                        let global_row = tr * hm.tile_size + r;
                        let global_col = tc * hm.tile_size + c;

                        if global_row == 0
                            || global_row >= hm.rows - 1
                            || global_col == 0
                            || global_col >= hm.cols - 1
                        {
                            continue;
                        }

                        sum += hm.get(global_row - 1, global_col) as i64;
                        sum += hm.get(global_row + 1, global_col) as i64;
                        sum += hm.get(global_row, global_col - 1) as i64;
                        sum += hm.get(global_row, global_col + 1) as i64;
                    }
                }
            }
        }
        std::hint::black_box(sum);
    });
    let gb_per_second: f64 = count_gb_per_sec(ticks, Some(4 * 2 * (hm.rows - 2) * (hm.cols - 2)));
    println!("tiled_walk_tiles_order: {:.1} GB/s", gb_per_second);
}
