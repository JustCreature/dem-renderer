use crate::utils::*;

use dem_io::Heightmap;
use terrain::{
    compute_shadow_neon, compute_shadow_neon_parallel, compute_shadow_neon_parallel_with_azimuth,
    compute_shadow_scalar, compute_shadow_scalar_with_azimuth, ShadowMask,
};

pub(crate) fn benchmark_shadow_mask_scalar(
    heightmap: &Heightmap,
    sun_elevation_rad: f32,
) -> ShadowMask {
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

pub(crate) fn benchmark_shadow_mask_scalar_with_azimuth(
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

pub(crate) fn benchmark_shadow_mask_scalar_with_azimuth_labeled(
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

pub(crate) fn benchmark_shadow_mask_neon(
    heightmap: &Heightmap,
    sun_elevation_rad: f32,
) -> ShadowMask {
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

pub(crate) fn benchmark_shadow_mask_neon_parallel(
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

pub(crate) fn benchmark_shadow_mask_neon_parallel_with_azimuth_labeled(
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
