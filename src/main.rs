use std::path::Path;

use dem_io::{Heightmap, TiledHeightmap};

static FREQ: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
const N: usize = 64 * 1024 * 1024;

fn shuffle(indices: &mut Vec<usize>) {
    let mut rng = 12345u64; // seed
    for i in (1..indices.len()).rev() {
        rng = rng
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let j = (rng >> 33) as usize % (i + 1);
        indices.swap(i, j);
    }
}

fn counter_frequency() -> f64 {
    *FREQ.get_or_init(|| {
        let t0 = profiling::now();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let t1 = profiling::now();
        (t1 - t0) as f64 / 0.1
    })
}

fn count_gb_per_sec(ticks: u64, bytes: Option<usize>) -> f64 {
    let freq = counter_frequency();
    let seconds = ticks as f64 / freq;
    let bytes = bytes.unwrap_or(N * std::mem::size_of::<f32>());
    let gb_per_sec = bytes as f64 / seconds / 1_000_000_000.0;
    gb_per_sec
}

#[cfg(target_arch = "aarch64")]
fn seq_read_simd(data: &[f32]) {
    use core::arch::aarch64::*;

    let (ticks, _) = profiling::timed("seq_read", || unsafe {
        let mut acc0 = vdupq_n_f32(0.0);
        let mut acc1 = vdupq_n_f32(0.0);
        let mut acc2 = vdupq_n_f32(0.0);
        let mut acc3 = vdupq_n_f32(0.0);

        for chunk in data.chunks_exact(16) {
            let ptr = chunk.as_ptr();

            let v0 = vld1q_f32(ptr);
            let v1 = vld1q_f32(ptr.add(4));
            let v2 = vld1q_f32(ptr.add(8));
            let v3 = vld1q_f32(ptr.add(12));

            acc0 = vaddq_f32(acc0, v0);
            acc1 = vaddq_f32(acc1, v1);
            acc2 = vaddq_f32(acc2, v2);
            acc3 = vaddq_f32(acc3, v3);
        }

        let sum01 = vaddq_f32(acc0, acc1);
        let sum23 = vaddq_f32(acc2, acc3);
        let total = vaddq_f32(sum01, sum23);

        let sum = vaddvq_f32(total);

        let remainder: f32 = data.chunks_exact(16).remainder().iter().sum();

        std::hint::black_box(sum + remainder);
    });

    let gb_per_sec = count_gb_per_sec(ticks, None);
    println!("seq_read_simd: {:.1} GB/s", gb_per_sec);
}

fn random_read_simd(data: &[f32]) {
    use core::arch::aarch64::*;

    let mut indices: Vec<usize> = (0..N).collect();
    shuffle(&mut indices);

    let (ticks, _) = profiling::timed("random_read_simd", || unsafe {
        let ptr = data.as_ptr();
        let mut acc0 = vdupq_n_f32(0.0);
        let mut acc1 = vdupq_n_f32(0.0);
        let mut acc2 = vdupq_n_f32(0.0);
        let mut acc3 = vdupq_n_f32(0.0);

        for chunk in indices.chunks_exact(16) {
            let v0 = vld1q_f32(
                [
                    *ptr.add(chunk[0]),
                    *ptr.add(chunk[1]),
                    *ptr.add(chunk[2]),
                    *ptr.add(chunk[3]),
                ]
                .as_ptr(),
            );
            let v1 = vld1q_f32(
                [
                    *ptr.add(chunk[4]),
                    *ptr.add(chunk[5]),
                    *ptr.add(chunk[6]),
                    *ptr.add(chunk[7]),
                ]
                .as_ptr(),
            );
            let v2 = vld1q_f32(
                [
                    *ptr.add(chunk[8]),
                    *ptr.add(chunk[9]),
                    *ptr.add(chunk[10]),
                    *ptr.add(chunk[11]),
                ]
                .as_ptr(),
            );
            let v3 = vld1q_f32(
                [
                    *ptr.add(chunk[12]),
                    *ptr.add(chunk[13]),
                    *ptr.add(chunk[14]),
                    *ptr.add(chunk[15]),
                ]
                .as_ptr(),
            );

            acc0 = vaddq_f32(acc0, v0);
            acc1 = vaddq_f32(acc1, v1);
            acc2 = vaddq_f32(acc2, v2);
            acc3 = vaddq_f32(acc3, v3);
        }

        let total = vaddq_f32(vaddq_f32(acc0, acc1), vaddq_f32(acc2, acc3));
        std::hint::black_box(vaddvq_f32(total));
    });

    let gb_per_sec = count_gb_per_sec(ticks, None);
    println!("random_read_simd: {:.1} GB/s", gb_per_sec);
}

fn seq_read(data: &[f32]) {
    let (ticks, _) = profiling::timed("seq_read", || {
        let mut sum = 0.0f32;
        for &x in data {
            sum += x;
        }
        std::hint::black_box(sum);
    });

    let gb_per_sec = count_gb_per_sec(ticks, None);
    println!("seq_read: {:.1} GB/s", gb_per_sec);
}

fn random_read(data: &[f32]) {
    let (ticks, _) = profiling::timed("seq_read", || {
        let mut sum = 0.0f32;
        let mut indices: Vec<usize> = (0..N).collect();
        shuffle(&mut indices);
        for i in 0..N {
            sum += data[indices[i]];
        }
        std::hint::black_box(sum);
    });

    let gb_per_sec = count_gb_per_sec(ticks, None);
    println!("random_read: {:.1} GB/s", gb_per_sec);
}

fn seq_write() {
    let (ticks, _) = profiling::timed("seq_read", || {
        let mut output = vec![0.0f32; N];
        for i in 0..N {
            output[i] = i as f32;
        }
        std::hint::black_box(output);
    });

    let gb_per_sec = count_gb_per_sec(ticks, None);
    println!("seq_write: {:.1} GB/s", gb_per_sec);
}

fn random_write() {
    let (ticks, _) = profiling::timed("seq_read", || {
        let mut output = vec![0.0f32; N];
        let mut indices: Vec<usize> = (0..N).collect();
        shuffle(&mut indices);
        for i in 0..N {
            output[indices[i]] = i as f32;
        }
        std::hint::black_box(output);
    });

    let gb_per_sec = count_gb_per_sec(ticks, None);
    println!("random_write: {:.1} GB/s", gb_per_sec);
}

fn bench_neighbours_rowmajor(hm: &Heightmap) {
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

fn bench_neighbours_tiled(hm: &TiledHeightmap) {
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

fn bench_neighbours_tiled_walk_tiles_order(hm: &TiledHeightmap) {
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

fn main() {
    println!("dem_renderer");
    let data: Vec<f32> = (0..N).map(|i| i as f32).collect();

    seq_read_simd(&data);
    println!("--------");
    seq_read(&data);
    println!("--------");
    random_read_simd(&data);
    println!("--------");
    random_read(&data);
    println!("--------");
    seq_write();
    println!("--------");
    random_write();
    println!("--------");

    let tile_path = Path::new("n47_e011_1arc_v3_bil/n47_e011_1arc_v3.bil");
    let (_, heightmap) =
        profiling::timed("build heightmap", || dem_io::parse_bil(tile_path).unwrap());

    let tiled_heightmap = dem_io::TiledHeightmap::from_heightmap(&heightmap, 128);

    assert_eq!(
        tiled_heightmap.get(100, 200),
        heightmap.data[100 * heightmap.cols + 200]
    );
    assert_eq!(
        tiled_heightmap.get(127, 200),
        heightmap.data[127 * heightmap.cols + 200]
    );
    assert_eq!(
        tiled_heightmap.get(128, 200),
        heightmap.data[128 * heightmap.cols + 200]
    );
    assert_eq!(
        tiled_heightmap.get(129, 200),
        heightmap.data[129 * heightmap.cols + 200]
    );

    println!("--------");

    // evict heightmap from cach
    let evict: Vec<i32> = (0..16 * 1024 * 1024).map(|i| i as i32).collect();
    std::hint::black_box(evict);

    bench_neighbours_rowmajor(&heightmap);

    // evict heightmap from cach
    let evict: Vec<i32> = (0..16 * 1024 * 1024).map(|i| i as i32).collect();
    std::hint::black_box(evict);

    bench_neighbours_tiled(&tiled_heightmap);

    // evict heightmap from cach
    let evict: Vec<i32> = (0..16 * 1024 * 1024).map(|i| i as i32).collect();
    std::hint::black_box(evict);

    bench_neighbours_tiled_walk_tiles_order(&tiled_heightmap);

    assert_eq!(tiled_heightmap.tiles().as_ptr() as usize % 4096, 0);
}
