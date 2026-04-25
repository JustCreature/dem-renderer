use std::path::Path;

use dem_io::Heightmap;
use terrain::{
    compute_normals_vector_par, compute_shadow_vector_par_with_azimuth, NormalMap, ShadowMask,
};

use crate::utils::counter_frequency;

pub(crate) fn render_3d_pic_cpu(tile_path: &Path) {
    let heightmap: Heightmap = dem_io::parse_bil(tile_path).unwrap();

    // Camera above the terrain, looking at Olperer
    // pixel space: origin at (col=2388, row=3341), z = terrain_height + 1800m
    // look_at: Olperer at (col=2527, row=3467), z = 3476m
    let dx = heightmap.dx_meters as f32;
    let dy = heightmap.dy_meters as f32;

    let pic_width = 2000;
    let pic_height = 900;

    // benchmark with google earth
    let cam_col = 2457.0f32;
    let cam_row = 3328.0f32;

    let cam = render_cpu::Camera::new(
        [cam_col * dx, cam_row * dy, 3341.0],
        [cam_col * dx + 19_627.0, cam_row * dy - 1_718.0, -131.0],
        70.0,
        pic_width as f32 / pic_height as f32,
    );

    let sun_dir = [0.4f32, 0.5f32, 0.7f32]; // [east, south, up] — morning sun NE
    let sun_azimuth_rad = (sun_dir[0]).atan2(-sun_dir[1]); // atan2(east, north)
    let sun_elevation_rad = sun_dir[2].atan2((sun_dir[0].powi(2) + sun_dir[1].powi(2)).sqrt());

    let normal_map: NormalMap = compute_normals_vector_par(&heightmap);

    let shadow_mask: ShadowMask = compute_shadow_vector_par_with_azimuth(
        &heightmap,
        sun_azimuth_rad,
        sun_elevation_rad,
        200.0,
    );

    let (ticks, fb) = profiling::timed("render_cpu[CPU|shadows_SCALAR_PARALLEL]", || {
        render_cpu::render_par(
            &cam,
            &heightmap,
            &normal_map,
            &shadow_mask,
            sun_dir,
            pic_width,
            pic_height,
            heightmap.dx_meters as f32 / 0.8,
            200_000.0,
        )
    });
    println!(
        "render_cpu CPU|shadows_SCALAR_PARALLEL {}x{}: {:.2}s",
        pic_width,
        pic_height,
        ticks as f64 / counter_frequency()
    );

    image::RgbImage::from_raw(pic_width, pic_height, fb)
        .unwrap()
        .save("artifacts/render_cpu_CPU_shadows_SCALAR_PARALLEL.png")
        .unwrap();
}

pub(crate) fn render_3d_pic_gpu(tile_path: &Path) {
    let heightmap: Heightmap = dem_io::parse_bil(tile_path).unwrap();

    // Camera above the terrain, looking at Olperer
    // pixel space: origin at (col=2388, row=3341), z = terrain_height + 1800m
    // look_at: Olperer at (col=2527, row=3467), z = 3476m
    let dx = heightmap.dx_meters as f32;
    let dy = heightmap.dy_meters as f32;

    let pic_width = 2000;
    let pic_height = 900;

    // benchmark with google earth
    let cam_col = 2457.0f32;
    let cam_row = 3328.0f32;

    let sun_dir = [0.4f32, 0.5f32, 0.7f32]; // [east, south, up] — morning sun NE
    let sun_azimuth_rad = (sun_dir[0]).atan2(-sun_dir[1]); // atan2(east, north)
    let sun_elevation_rad = sun_dir[2].atan2((sun_dir[0].powi(2) + sun_dir[1].powi(2)).sqrt());

    let normal_map: NormalMap = compute_normals_vector_par(&heightmap);

    let shadow_mask: ShadowMask = compute_shadow_vector_par_with_azimuth(
        &heightmap,
        sun_azimuth_rad,
        sun_elevation_rad,
        200.0,
    );

    let gpu_ctx = render_gpu::GpuContext::new();

    let (ticks, fb) = profiling::timed("render_gpu[GPU]", || {
        render_gpu::render_gpu_texture(
            &gpu_ctx,
            [cam_col * dx, cam_row * dy, 3341.0],
            [cam_col * dx + 19_627.0, cam_row * dy - 1_718.0, -131.0],
            70.0,
            pic_width as f32 / pic_height as f32,
            &heightmap,
            &normal_map,
            &shadow_mask,
            sun_dir,
            pic_width,
            pic_height,
            heightmap.dx_meters as f32 / 0.8,
            200_000.0,
            0,
        )
    });
    println!(
        "render_gpu GPU {}x{}: {:.2}s",
        pic_width,
        pic_height,
        ticks as f64 / counter_frequency()
    );

    image::RgbaImage::from_raw(pic_width, pic_height, fb)
        .unwrap()
        .save("artifacts/render_gpu_GPU.png")
        .unwrap();
}
