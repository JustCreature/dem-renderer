mod benchmarks;
mod frame_render_final;
mod render_gif;
mod system_info;
mod utils;

use std::path::Path;

use crate::benchmarks::*;
use frame_render_final::{render_3d_pic_cpu, render_3d_pic_gpu};
use utils::*;

use dem_io::{Heightmap, TiledHeightmap};
use terrain::{NormalMap, ShadowMask};

fn main() {
    system_info::print_system_info();
    println!("dem_renderer");
    let data: Vec<f32> = (0..N).map(|i| i as f32).collect();

    seq_read_vector(&data);
    println!("--------");
    seq_read(&data);
    println!("--------");
    random_read_vector(&data);
    println!("--------");
    random_read(&data);
    println!("--------");
    seq_write();
    println!("--------");
    random_write();
    println!("--------");

    let tile_path = Path::new("n47_e011_1arc_v3_bil/n47_e011_1arc_v3.bil");
    let (_, heightmap): (u64, Heightmap) =
        profiling::timed("build heightmap", || dem_io::parse_bil(tile_path).unwrap());

    let tiled_heightmap: TiledHeightmap = dem_io::TiledHeightmap::from_heightmap(&heightmap, 128);

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

    let normal_map: NormalMap = benchmark_normal_map_scalar(&heightmap);

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

    let gpu_ctx = render_gpu::GpuContext::new();

    let (ticks, _normal_map_gpu) = profiling::timed("compute_normals_gpu", || {
        render_gpu::compute_normals_gpu(&gpu_ctx, &heightmap)
    });
    println!(
        "compute_normals_gpu: {:.2}s",
        ticks as f64 / counter_frequency()
    );

    // evict heightmap from cach
    let evict: Vec<i32> = (0..100 * 1024 * 1024).map(|i| i as i32).collect();
    std::hint::black_box(evict);

    // -- SHADOWS
    println!("---------- SHADOWS ----------");

    let sun_elevation_rad_const: f32 = 20.0f32.to_radians();

    let shadow_mask: ShadowMask = benchmark_shadow_mask_scalar(&heightmap, sun_elevation_rad_const);

    // evict heightmap from cach
    let evict: Vec<i32> = (0..100 * 1024 * 1024).map(|i| i as i32).collect();
    std::hint::black_box(evict);

    benchmark_shadow_mask_vector(&heightmap, sun_elevation_rad_const);

    // evict heightmap from cach
    let evict: Vec<i32> = (0..100 * 1024 * 1024).map(|i| i as i32).collect();
    std::hint::black_box(evict);

    benchmark_shadow_mask_vector_parallel(&heightmap, sun_elevation_rad_const);

    // evict heightmap from cach
    let evict: Vec<i32> = (0..100 * 1024 * 1024).map(|i| i as i32).collect();
    std::hint::black_box(evict);

    let sun_azimuth_rad_west: f32 = 225f32.to_radians();
    benchmark_shadow_mask_scalar_with_azimuth(
        &heightmap,
        sun_azimuth_rad_west,
        sun_elevation_rad_const,
    );

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

    benchmark_shadow_mask_vector_parallel_with_azimuth_labeled(
        &heightmap,
        270f32.to_radians(),
        10f32.to_radians(),
        "sunset",
    );

    // evict heightmap from cach
    let evict: Vec<i32> = (0..100 * 1024 * 1024).map(|i| i as i32).collect();
    std::hint::black_box(evict);

    let (ticks, _shadow_mask_gpu) = profiling::timed("compute_shadow_gpu", || {
        render_gpu::compute_shadow_gpu(&gpu_ctx, &heightmap, sun_elevation_rad_const)
    });
    println!(
        "compute_shadow_gpu: {:.2}s",
        ticks as f64 / counter_frequency()
    );

    // evict heightmap from cach
    let evict: Vec<i32> = (0..100 * 1024 * 1024).map(|i| i as i32).collect();
    std::hint::black_box(evict);

    // -- Camera CPU Renderer

    println!("---------- Camera CPU Renderer ----------");

    // Camera above the terrain, looking at Olperer
    // pixel space: origin at (col=2388, row=3341), z = terrain_height + 1800m
    // look_at: Olperer at (col=2527, row=3467), z = 3476m
    let dx = heightmap.dx_meters as f32;
    let dy = heightmap.dy_meters as f32;

    let mut pic_width = 2000;
    let mut pic_height = 900;

    // // look at tux
    // let cam = render_cpu::Camera::new(
    //     [
    //         2388.0 * dx,
    //         3341.0 * dy,
    //         heightmap.data[3341 * heightmap.cols + 2388] as f32 + 1000.0,
    //     ],
    //     [2371.0 * dx, 3409.0 * dy, 3449.0],
    //     70.0,
    //     pic_width as f32 / pic_height as f32,
    // );

    // // look at valley 180
    // let cam = render_cpu::Camera::new(
    //     [
    //         2388.0 * dx,
    //         3341.0 * dy,
    //         heightmap.data[3341 * heightmap.cols + 2388] as f32 + 1000.0,
    //     ],
    //     [2371.0 * dx, 3409.0 * dy, 3449.0],
    //     90.0,
    //     1.0, // square image for now
    // );

    // // look at 90 deg west
    // let cam = render_cpu::Camera::new(
    //     [
    //         2388.0 * dx,
    //         3341.0 * dy,
    //         heightmap.data[3341 * heightmap.cols + 2388] as f32 + 1000.0,
    //     ],
    //     [2371.0 * dx - 20_000.0, 3409.0 * dy, 2000.0],
    //     60.0,
    //     1.0, // square image for now
    // );

    // benchmark with google earth
    let cam_col = 2457.0f32;
    let cam_row = 3328.0f32;

    // let pic_width = 2000;
    // let pic_height = 900;

    let mut cam = render_cpu::Camera::new(
        [cam_col * dx, cam_row * dy, 3341.0],
        [cam_col * dx + 19_627.0, cam_row * dy - 1_718.0, -131.0],
        70.0,
        pic_width as f32 / pic_height as f32,
    );

    // Center pixel should point roughly at look_at
    let ray_center = cam.ray_for_pixel(500, 500, 1000, 1000);
    println!("center ray dir: {:?}", ray_center.dir);

    // Top-left pixel should point up-left relative to center
    let ray_tl = cam.ray_for_pixel(0, 0, 1000, 1000);
    println!("top-left ray dir: {:?}", ray_tl.dir);

    let hit = render_cpu::raymarch(
        &ray_center,
        &heightmap,
        heightmap.dx_meters as f32 / 1.0,
        200_000.0,
    );
    println!("hit: {:?}", hit);

    let sun_dir = [0.4f32, 0.5f32, 0.7f32]; // [east, south, up] — morning sun NE

    let (ticks, fb) = profiling::timed("render_cpu_parallel", || {
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
        "render_cpu PARALLEL {}x{}: {:.2}s",
        pic_width,
        pic_height,
        ticks as f64 / counter_frequency()
    );

    image::RgbImage::from_raw(pic_width, pic_height, fb)
        .unwrap()
        .save("artifacts/render_cpu[parallel].png")
        .unwrap();

    let (ticks, fb) = profiling::timed("render_cpu[VECTOR]", || {
        render_cpu::render_vector(
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
        "render_cpu [VECTOR] {}x{}: {:.2}s",
        pic_width,
        pic_height,
        ticks as f64 / counter_frequency()
    );
    image::RgbImage::from_raw(pic_width, pic_height, fb)
        .unwrap()
        .save("artifacts/render_cpu[VECTOR].png")
        .unwrap();

    let (ticks, fb) = profiling::timed("render_cpu_VECTOR_parallel", || {
        render_cpu::render_vector_par(
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
        "render_cpu VECTOR PARALLEL {}x{}: {:.2}s",
        pic_width,
        pic_height,
        ticks as f64 / counter_frequency()
    );

    image::RgbImage::from_raw(pic_width, pic_height, fb)
        .unwrap()
        .save("artifacts/render_cpu[VECTOR_parallel].png")
        .unwrap();

    render_3d_pic_cpu(tile_path);
    render_3d_pic_gpu(tile_path);

    pic_width = 8000;
    pic_height = 2667;
    cam = render_cpu::Camera::new(
        [cam_col * dx, cam_row * dy, 3341.0],
        [cam_col * dx + 19_627.0, cam_row * dy - 1_718.0, -131.0],
        100.0,
        pic_width as f32 / pic_height as f32,
    );
    let step_size: f32 = 1.0;

    let (ticks, fb) = profiling::timed("render_cpu", || {
        render_cpu::render_par(
            &cam,
            &heightmap,
            &normal_map,
            &shadow_mask,
            sun_dir,
            pic_width,
            pic_height,
            heightmap.dx_meters as f32 / step_size,
            200_000.0,
        )
    });
    println!(
        "render_cpu BIG PARALLEL {}x{}: {:.2}s",
        pic_width,
        pic_height,
        ticks as f64 / counter_frequency()
    );
    image::RgbImage::from_raw(pic_width, pic_height, fb)
        .unwrap()
        .save("artifacts/render_cpu.png")
        .unwrap();

    // -- Camera GPU Renderer

    println!("---------- Camera GPU Renderer ----------");

    let (ticks, fb) = profiling::timed("render_gpu", || {
        render_gpu::render_gpu_buffer(
            &gpu_ctx,
            [cam_col * dx, cam_row * dy, 3341.0],
            [cam_col * dx + 19_627.0, cam_row * dy - 1_718.0, -131.0],
            100.0,
            pic_width as f32 / pic_height as f32,
            &heightmap,
            &normal_map,
            &shadow_mask,
            sun_dir,
            pic_width,
            pic_height,
            heightmap.dx_meters as f32 / step_size,
            200_000.0,
        )
    });
    println!(
        "render_gpu BIG {}x{}: {:.2}s",
        pic_width,
        pic_height,
        ticks as f64 / counter_frequency()
    );

    image::RgbaImage::from_raw(pic_width, pic_height, fb)
        .unwrap()
        .save("artifacts/render_gpu.png")
        .unwrap();

    let (ticks, fb) = profiling::timed("render_gpu[texture]", || {
        render_gpu::render_gpu_texture(
            &gpu_ctx,
            [cam_col * dx, cam_row * dy, 3341.0],
            [cam_col * dx + 19_627.0, cam_row * dy - 1_718.0, -131.0],
            100.0,
            pic_width as f32 / pic_height as f32,
            &heightmap,
            &normal_map,
            &shadow_mask,
            sun_dir,
            pic_width,
            pic_height,
            heightmap.dx_meters as f32 / step_size,
            200_000.0,
        )
    });
    println!(
        "render_gpu BIG [texture] {}x{}: {:.2}s",
        pic_width,
        pic_height,
        ticks as f64 / counter_frequency()
    );

    image::RgbaImage::from_raw(pic_width, pic_height, fb)
        .unwrap()
        .save("artifacts/render_gpu[texture].png")
        .unwrap();

    let (ticks, fb) = profiling::timed("render_gpu[combined]", || {
        render_gpu::render_gpu_combined(
            &gpu_ctx,
            &heightmap,
            &shadow_mask,
            [cam_col * dx, cam_row * dy, 3341.0],
            [cam_col * dx + 19_627.0, cam_row * dy - 1_718.0, -131.0],
            100.0,
            pic_width as f32 / pic_height as f32,
            sun_dir,
            pic_width,
            pic_height,
            heightmap.dx_meters as f32 / step_size,
            200_000.0,
        )
    });
    println!(
        "render_gpu BIG [combined] {}x{}: {:.2}s",
        pic_width,
        pic_height,
        ticks as f64 / counter_frequency()
    );
    image::RgbaImage::from_raw(pic_width, pic_height, fb)
        .unwrap()
        .save("artifacts/render_gpu[combined].png")
        .unwrap();

    // -- Multi-frame benchmark
    println!("---------- Multi-frame benchmark ----------");
    // // commeted out since it takes about 17 seconds to run
    // benchmark_multi_frame_cpu(&heightmap, &normal_map, &shadow_mask);
    benchmark_multi_frame_gpu_separate(&gpu_ctx, &heightmap, &normal_map, &shadow_mask);
    benchmark_multi_frame_gpu_combined(&gpu_ctx, &heightmap, &shadow_mask);
    // GpuScene takes ctx by value (owns it) — create a dedicated one
    benchmark_multi_frame_gpu_scene(render_gpu::GpuContext::new(), &heightmap, &shadow_mask);

    render_gif::render_gif(tile_path);

    // -- Phase 6 benchmarks
    println!("---------- Phase 6 Benchmarks ----------");

    // evict before phase 6
    let evict: Vec<i32> = (0..100 * 1024 * 1024).map(|i| i as i32).collect();
    std::hint::black_box(evict);

    bench_tile_size_sweep(&heightmap);
    bench_thread_count_scaling(&heightmap);
    bench_thread_count_scaling_readonly(&heightmap);

    run_phase_6_benchmarks(&heightmap, &normal_map, &data);

    // -- Valley render
    // Camera: 47°03'52.84"N 11°42'26.24"E, alt 3284m, heading 165°, tilt 72° from nadir
    // Google Earth cursor alt 2339m → used as look-at altitude
    // row = (48 - 47.064678) * 3600 = 3367, col = (11.707289 - 11) * 3600 = 2546
    // moved 200m back (opposite heading) and tilted down to 72° (was 78°)
    {
        let cam_col_v = 2546.0f32;
        let cam_row_v = 3367.0f32;
        let cam_alt_v = 3284.0f32;
        let look_alt_v = 2339.0f32;
        let heading_rad_v = 165.0f32.to_radians();
        // move 200m back: opposite heading direction
        let cam_x_v = cam_col_v * dx - 800.0 * heading_rad_v.sin();
        let cam_y_v = cam_row_v * dy + 800.0 * heading_rad_v.cos(); // cos(165°)<0 → moves north
                                                                    // tilt down to 72° from nadir (was 78°): shorter horizontal lookahead
        let horiz_v = (cam_alt_v - look_alt_v) * 77.0f32.to_radians().tan();
        let look_x_v = cam_x_v + horiz_v * heading_rad_v.sin();
        let look_y_v = cam_y_v + horiz_v * (-heading_rad_v.cos());

        let (ticks, fb) = profiling::timed("render valley", || {
            render_gpu::render_gpu_texture(
                &gpu_ctx,
                [cam_x_v, cam_y_v, cam_alt_v],
                [look_x_v, look_y_v, look_alt_v],
                70.0,
                8000.0 / 2667.0,
                &heightmap,
                &normal_map,
                &shadow_mask,
                sun_dir,
                8000,
                2667,
                heightmap.dx_meters as f32 / 0.8,
                200_000.0,
            )
        });
        println!("valley render: {:.2}s", ticks as f64 / counter_frequency());

        image::RgbaImage::from_raw(8000, 2667, fb)
            .unwrap()
            .save("artifacts/valley.png")
            .unwrap();
    }
}

fn run_phase_6_benchmarks(heightmap: &Heightmap, normal_map: &NormalMap, data: &Vec<f32>) {
    // evict before exp 4
    let evict: Vec<i32> = (0..100 * 1024 * 1024).map(|i| i as i32).collect();
    std::hint::black_box(evict);

    bench_aos_vs_soa(&normal_map);

    // evict before exp 5
    let evict: Vec<i32> = (0..100 * 1024 * 1024).map(|i| i as i32).collect();
    std::hint::black_box(evict);

    bench_morton_vs_rowmajor(&heightmap);

    // evict before exp 6/7
    let evict: Vec<i32> = (0..100 * 1024 * 1024).map(|i| i as i32).collect();
    std::hint::black_box(evict);

    bench_software_prefetch(&data);

    let evict: Vec<i32> = (0..100 * 1024 * 1024).map(|i| i as i32).collect();
    std::hint::black_box(evict);

    bench_vector_accumulators(&normal_map);

    let evict: Vec<i32> = (0..100 * 1024 * 1024).map(|i| i as i32).collect();
    std::hint::black_box(evict);

    bench_gather_ray_packets(&heightmap);

    let evict: Vec<i32> = (0..100 * 1024 * 1024).map(|i| i as i32).collect();
    std::hint::black_box(evict);

    bench_tlb_sweep(&data);
}
