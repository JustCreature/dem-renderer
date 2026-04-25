use crate::utils::*;

use dem_io::Heightmap;
use terrain::{
    compute_normals_neon, compute_normals_neon_parallel, compute_normals_neon_tiled,
    compute_normals_neon_tiled_parallel, compute_normals_scalar, NormalMap,
};

pub(crate) fn benchmark_normal_map_scalar(heightmap: &Heightmap) -> NormalMap {
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

pub(crate) fn benchmark_normal_map_vectorised(heightmap: &Heightmap) -> NormalMap {
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

pub(crate) fn benchmark_normal_map_parallel_vectorised(heightmap: &Heightmap) -> NormalMap {
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

pub(crate) fn benchmark_normal_map_tiled_vectorised(
    tiled_hm: &dem_io::TiledHeightmap,
) -> NormalMap {
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

pub(crate) fn benchmark_normal_map_tiled_parallel_vectorised(
    tiled_hm: &dem_io::TiledHeightmap,
) -> NormalMap {
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
