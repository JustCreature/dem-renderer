use std::path::Path;

use dem_io::{Heightmap, TiledHeightmap};
use terrain::{
    compute_normals_neon, compute_normals_neon_8, compute_normals_neon_parallel,
    compute_normals_neon_tiled, compute_normals_neon_tiled_parallel, compute_normals_scalar,
    compute_shadow_neon, compute_shadow_neon_parallel, compute_shadow_neon_parallel_with_azimuth,
    compute_shadow_scalar, compute_shadow_scalar_branchless, compute_shadow_scalar_with_azimuth,
    NormalMap, ShadowMask,
};

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

fn create_cropped_image(heightmap: &Heightmap, normal_map: &NormalMap) {
    let r_start = 1000;
    let c_start = 1500;
    let height = 500;
    let width = 500;

    let nz = &normal_map.nz;
    let pixels: Vec<u8> = (r_start..r_start + height)
        .flat_map(|r| {
            (c_start..c_start + width).map(move |c| (nz[r * heightmap.cols + c] * 255.0) as u8)
        })
        .collect();

    image::GrayImage::from_raw(width as u32, height as u32, pixels)
        .unwrap()
        .save("artifacts/normals_crop.png")
        .unwrap();
}

fn benchmark_normal_map_scalar(heightmap: &Heightmap) -> NormalMap {
    let (ticks, normal_map) = profiling::timed("build normals map from row-major hm", || {
        compute_normals_scalar(&heightmap)
    });
    let gb_per_second: f64 = count_gb_per_sec(
        ticks,
        Some(4 * 2 * heightmap.rows * heightmap.cols + 3 * 4 * heightmap.rows * heightmap.cols),
    );
    println!(
        "compute_normals_scalar_from_row_major_hm: {:.1} GB/s",
        gb_per_second
    );

    let pixels: Vec<u8> = normal_map.nz.iter().map(|&v| (v * 255.0) as u8).collect();
    image::GrayImage::from_raw(heightmap.cols as u32, heightmap.rows as u32, pixels)
        .unwrap()
        .save("artifacts/normals_nz.png")
        .unwrap();

    create_cropped_image(&heightmap, &normal_map);

    normal_map
}

fn benchmark_normal_map_vectorised(heightmap: &Heightmap) -> NormalMap {
    let (ticks, normal_map_vec) = profiling::timed(
        "[vectotized] build normals map from row-major hm",
        || unsafe { compute_normals_neon(&heightmap) },
    );
    let gb_per_second: f64 = count_gb_per_sec(
        ticks,
        Some(4 * 2 * heightmap.rows * heightmap.cols + 3 * 4 * heightmap.rows * heightmap.cols),
    );
    println!(
        "[vectotized]compute_normals_scalar_from_row_major_hm: {:.1} GB/s",
        gb_per_second
    );

    let pixels: Vec<u8> = normal_map_vec
        .nz
        .iter()
        .map(|&v| (v * 255.0) as u8)
        .collect();
    image::GrayImage::from_raw(heightmap.cols as u32, heightmap.rows as u32, pixels)
        .unwrap()
        .save("artifacts/[vectotized]normals_nz.png")
        .unwrap();

    normal_map_vec
}

fn benchmark_normal_map_parallel_vectorised(heightmap: &Heightmap) -> NormalMap {
    let (ticks, normal_map_vec) = profiling::timed(
        "[parallel_vectotized] build normals map from row-major hm",
        || unsafe { compute_normals_neon_parallel(&heightmap) },
    );
    let gb_per_second: f64 = count_gb_per_sec(
        ticks,
        Some(4 * 2 * heightmap.rows * heightmap.cols + 3 * 4 * heightmap.rows * heightmap.cols),
    );
    println!(
        "[parallel_vectotized]compute_normals_scalar_from_row_major_hm: {:.1} GB/s",
        gb_per_second
    );

    let pixels: Vec<u8> = normal_map_vec
        .nz
        .iter()
        .map(|&v| (v * 255.0) as u8)
        .collect();
    image::GrayImage::from_raw(heightmap.cols as u32, heightmap.rows as u32, pixels)
        .unwrap()
        .save("artifacts/[parallel_vectotized]normals_nz.png")
        .unwrap();

    normal_map_vec
}

fn benchmark_normal_map_tiled_vectorised(tiled_hm: &dem_io::TiledHeightmap) -> NormalMap {
    let (ticks, normal_map) = profiling::timed(
        "[tiled_vectorised] build normals map from tiled hm",
        || unsafe { compute_normals_neon_tiled(tiled_hm) },
    );
    let gb_per_second: f64 = count_gb_per_sec(
        ticks,
        Some(4 * 2 * tiled_hm.rows * tiled_hm.cols + 3 * 4 * tiled_hm.rows * tiled_hm.cols),
    );
    println!(
        "[tiled_vectorised]compute_normals_neon_tiled: {:.1} GB/s",
        gb_per_second
    );
    normal_map
}

fn benchmark_normal_map_tiled_parallel_vectorised(tiled_hm: &dem_io::TiledHeightmap) -> NormalMap {
    let (ticks, normal_map) = profiling::timed(
        "[tiled_parallel_vectorised] build normals map from tiled hm",
        || unsafe { compute_normals_neon_tiled_parallel(tiled_hm) },
    );
    let gb_per_second: f64 = count_gb_per_sec(
        ticks,
        Some(4 * 2 * tiled_hm.rows * tiled_hm.cols + 3 * 4 * tiled_hm.rows * tiled_hm.cols),
    );
    println!(
        "[tiled_parallel_vectorised]compute_normals_neon_tiled_parallel: {:.1} GB/s",
        gb_per_second
    );
    normal_map
}

fn create_rgb_png(heightmap: &Heightmap, normal_map: &NormalMap) {
    let pixels: Vec<u8> = (0..heightmap.rows * heightmap.cols)
        .flat_map(|i| {
            let nx = ((normal_map.nx[i] + 1.0) * 0.5 * 255.0) as u8;
            let ny = ((normal_map.ny[i] + 1.0) * 0.5 * 255.0) as u8;
            let nz = (normal_map.nz[i] * 255.0) as u8;
            [nx, ny, nz]
        })
        .collect();

    image::RgbImage::from_raw(heightmap.cols as u32, heightmap.rows as u32, pixels)
        .unwrap()
        .save("artifacts/normals_rgb.png")
        .unwrap();
}

fn benchmark_shadow_mask_scalar(heightmap: &Heightmap, sun_elevation_rad: f32) -> ShadowMask {
    let (ticks, shadow_mask) =
        profiling::timed("build shadow mask SCALAR from row-major hm", || {
            compute_shadow_scalar(&heightmap, sun_elevation_rad)
        });
    let gb_per_second: f64 = count_gb_per_sec(
        ticks,
        Some(2 * heightmap.rows * heightmap.cols + 4 * heightmap.rows * heightmap.cols),
    );
    println!(
        "compute_shadows_scalar_from_row_major_hm: {:.1} GB/s",
        gb_per_second
    );

    let pixels: Vec<u8> = shadow_mask
        .data
        .iter()
        .map(|&v| (v * 255.0) as u8)
        .collect();
    image::GrayImage::from_raw(heightmap.cols as u32, heightmap.rows as u32, pixels)
        .unwrap()
        .save("artifacts/shadow_mask_scalar_west_to_east.png")
        .unwrap();

    shadow_mask
}

fn benchmark_shadow_mask_scalar_with_azimuth(
    heightmap: &Heightmap,
    sun_azimuth_rad: f32,
    sun_elevation_rad: f32,
) -> ShadowMask {
    let (ticks, shadow_mask) = profiling::timed(
        "build shadow mask SCALAR with_azimuth from row-major hm",
        || compute_shadow_scalar_with_azimuth(&heightmap, sun_azimuth_rad, sun_elevation_rad),
    );
    let gb_per_second: f64 = count_gb_per_sec(
        ticks,
        Some(2 * heightmap.rows * heightmap.cols + 4 * heightmap.rows * heightmap.cols),
    );
    println!(
        "compute_shadows_scalar_with_azimuth_from_row_major_hm: {:.1} GB/s",
        gb_per_second
    );

    let pixels: Vec<u8> = shadow_mask
        .data
        .iter()
        .map(|&v| (v * 255.0) as u8)
        .collect();
    image::GrayImage::from_raw(heightmap.cols as u32, heightmap.rows as u32, pixels)
        .unwrap()
        .save("artifacts/shadow_mask_scalar_with_azimuth_225_grad_west_to_east.png")
        .unwrap();

    shadow_mask
}

fn benchmark_shadow_mask_scalar_with_azimuth_labeled(
    heightmap: &Heightmap,
    sun_azimuth_rad: f32,
    sun_elevation_rad: f32,
    cur_label: &str,
) -> ShadowMask {
    let (ticks, shadow_mask) = profiling::timed(
        &format!(
            "[[ {} ]] build shadow mask SCALAR with_azimuth from row-major hm",
            cur_label
        ),
        || compute_shadow_scalar_with_azimuth(&heightmap, sun_azimuth_rad, sun_elevation_rad),
    );
    let gb_per_second: f64 = count_gb_per_sec(
        ticks,
        Some(2 * heightmap.rows * heightmap.cols + 4 * heightmap.rows * heightmap.cols),
    );
    println!(
        "compute_shadows_scalar_with_azimuth_labeled_from_row_major_hm: {:.1} GB/s",
        gb_per_second
    );

    let pixels: Vec<u8> = shadow_mask
        .data
        .iter()
        .map(|&v| (v * 255.0) as u8)
        .collect();
    image::GrayImage::from_raw(heightmap.cols as u32, heightmap.rows as u32, pixels)
        .unwrap()
        .save(&format!(
            "artifacts/[{}]_shadow_mask_scalar_with_azimuth_west_to_east.png",
            cur_label
        ))
        .unwrap();

    shadow_mask
}

fn benchmark_shadow_mask_neon(heightmap: &Heightmap, sun_elevation_rad: f32) -> ShadowMask {
    let (ticks, shadow_mask) =
        profiling::timed("build shadow mask NEON from row-major hm", || unsafe {
            compute_shadow_neon(&heightmap, sun_elevation_rad)
        });
    let gb_per_second: f64 = count_gb_per_sec(
        ticks,
        Some(2 * heightmap.rows * heightmap.cols + 4 * heightmap.rows * heightmap.cols),
    );
    println!(
        "compute_shadows_neon_from_row_major_hm: {:.1} GB/s",
        gb_per_second
    );

    let pixels: Vec<u8> = shadow_mask
        .data
        .iter()
        .map(|&v| (v * 255.0) as u8)
        .collect();
    image::GrayImage::from_raw(heightmap.cols as u32, heightmap.rows as u32, pixels)
        .unwrap()
        .save("artifacts/shadow_mask_neon_west_to_east.png")
        .unwrap();

    shadow_mask
}

fn benchmark_shadow_mask_neon_parallel(
    heightmap: &Heightmap,
    sun_elevation_rad: f32,
) -> ShadowMask {
    let (ticks, shadow_mask) = profiling::timed(
        "build shadow mask NEON parallel from row-major hm",
        || unsafe { compute_shadow_neon_parallel(&heightmap, sun_elevation_rad) },
    );
    let gb_per_second: f64 = count_gb_per_sec(
        ticks,
        Some(2 * heightmap.rows * heightmap.cols + 4 * heightmap.rows * heightmap.cols),
    );
    println!(
        "compute_shadows_neon_parallel_from_row_major_hm: {:.1} GB/s",
        gb_per_second
    );

    let pixels: Vec<u8> = shadow_mask
        .data
        .iter()
        .map(|&v| (v * 255.0) as u8)
        .collect();
    image::GrayImage::from_raw(heightmap.cols as u32, heightmap.rows as u32, pixels)
        .unwrap()
        .save("artifacts/shadow_mask_neon_parallel_west_to_east.png")
        .unwrap();

    shadow_mask
}

fn benchmark_shadow_mask_neon_parallel_with_azimuth_labeled(
    heightmap: &Heightmap,
    sun_azimuth_rad: f32,
    sun_elevation_rad: f32,
    cur_label: &str,
) -> ShadowMask {
    let (ticks, shadow_mask) = profiling::timed(
        &format!(
            "[[ {} ]] build shadow mask NEON parallel from row-major hm",
            cur_label
        ),
        || unsafe {
            compute_shadow_neon_parallel_with_azimuth(
                &heightmap,
                sun_azimuth_rad,
                sun_elevation_rad,
            )
        },
    );
    let gb_per_second: f64 = count_gb_per_sec(
        ticks,
        Some(2 * heightmap.rows * heightmap.cols + 4 * heightmap.rows * heightmap.cols),
    );
    println!(
        "compute_shadows_neon_parallel_labeled_from_row_major_hm: {:.1} GB/s",
        gb_per_second
    );

    let pixels: Vec<u8> = shadow_mask
        .data
        .iter()
        .map(|&v| (v * 255.0) as u8)
        .collect();
    image::GrayImage::from_raw(heightmap.cols as u32, heightmap.rows as u32, pixels)
        .unwrap()
        .save(&format!(
            "artifacts/[{}]_shadow_mask_neon_parallel_west_to_east.png",
            cur_label
        ))
        .unwrap();

    shadow_mask
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

    println!("--------");

    // evict heightmap from cach
    let evict: Vec<i32> = (0..100 * 1024 * 1024).map(|i| i as i32).collect();
    std::hint::black_box(evict);

    // -- NORMALS
    println!("---------- NORMALS ----------");

    let normal_map = benchmark_normal_map_scalar(&heightmap);

    create_rgb_png(&heightmap, &normal_map);

    // evict heightmap from cach
    let evict: Vec<i32> = (0..100 * 1024 * 1024).map(|i| i as i32).collect();
    std::hint::black_box(evict);

    benchmark_normal_map_vectorised(&heightmap);

    // evict heightmap from cach
    let evict: Vec<i32> = (0..100 * 1024 * 1024).map(|i| i as i32).collect();
    std::hint::black_box(evict);

    benchmark_normal_map_parallel_vectorised(&heightmap);

    // evict heightmap from cach
    let evict: Vec<i32> = (0..100 * 1024 * 1024).map(|i| i as i32).collect();
    std::hint::black_box(evict);

    benchmark_normal_map_tiled_vectorised(&tiled_heightmap);

    // evict heightmap from cach
    let evict: Vec<i32> = (0..100 * 1024 * 1024).map(|i| i as i32).collect();
    std::hint::black_box(evict);

    benchmark_normal_map_tiled_parallel_vectorised(&tiled_heightmap);

    // evict heightmap from cach
    let evict: Vec<i32> = (0..100 * 1024 * 1024).map(|i| i as i32).collect();
    std::hint::black_box(evict);

    // -- SHADOWS
    println!("---------- SHADOWS ----------");

    let sun_elevation_rad: f32 = 20.0f32.to_radians();

    let shadow_mask = benchmark_shadow_mask_scalar(&heightmap, sun_elevation_rad);

    // evict heightmap from cach
    let evict: Vec<i32> = (0..100 * 1024 * 1024).map(|i| i as i32).collect();
    std::hint::black_box(evict);

    benchmark_shadow_mask_neon(&heightmap, sun_elevation_rad);

    // evict heightmap from cach
    let evict: Vec<i32> = (0..100 * 1024 * 1024).map(|i| i as i32).collect();
    std::hint::black_box(evict);

    benchmark_shadow_mask_neon_parallel(&heightmap, sun_elevation_rad);

    // evict heightmap from cach
    let evict: Vec<i32> = (0..100 * 1024 * 1024).map(|i| i as i32).collect();
    std::hint::black_box(evict);

    let sun_azimuth_rad_west: f32 = 225f32.to_radians();
    benchmark_shadow_mask_scalar_with_azimuth(&heightmap, sun_azimuth_rad_west, sun_elevation_rad);

    // evict heightmap from cach
    let evict: Vec<i32> = (0..100 * 1024 * 1024).map(|i| i as i32).collect();
    std::hint::black_box(evict);

    benchmark_shadow_mask_scalar_with_azimuth_labeled(
        &heightmap,
        90f32.to_radians(),
        15f32.to_radians(),
        "sunrise",
    );

    // evict heightmap from cach
    let evict: Vec<i32> = (0..100 * 1024 * 1024).map(|i| i as i32).collect();
    std::hint::black_box(evict);

    benchmark_shadow_mask_scalar_with_azimuth_labeled(
        &heightmap,
        270f32.to_radians(),
        10f32.to_radians(),
        "sunset",
    );

    // evict heightmap from cach
    let evict: Vec<i32> = (0..100 * 1024 * 1024).map(|i| i as i32).collect();
    std::hint::black_box(evict);

    benchmark_shadow_mask_neon_parallel_with_azimuth_labeled(
        &heightmap,
        270f32.to_radians(),
        10f32.to_radians(),
        "sunset",
    );

    // evict heightmap from cach
    let evict: Vec<i32> = (0..100 * 1024 * 1024).map(|i| i as i32).collect();
    std::hint::black_box(evict);

    // -- Camera CPU Renderer

    println!("---------- Camera CPU Renderer ----------");

    // Camera above the terrain, looking at Olperer
    // pixel space: origin at (col=2388, row=3341), z = terrain_height + 1800m
    // look_at: Olperer at (col=2527, row=3467), z = 3476m
    let cam = render_cpu::Camera::new(
        [
            2388.0 * 21.06,
            3341.0 * 30.87,
            heightmap.data[3341 * heightmap.cols + 2388] as f32 + 1800.0,
        ],
        [2527.0 * 21.06, 3467.0 * 30.87, 3476.0],
        60.0,
        1.0, // square image for now
    );

    // Center pixel should point roughly at look_at
    let ray_center = cam.ray_for_pixel(500, 500, 1000, 1000);
    println!("center ray dir: {:?}", ray_center.dir);

    // Top-left pixel should point up-left relative to center
    let ray_tl = cam.ray_for_pixel(0, 0, 1000, 1000);
    println!("top-left ray dir: {:?}", ray_tl.dir);
}
