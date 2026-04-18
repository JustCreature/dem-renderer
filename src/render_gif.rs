use std::fs::File;
use std::path::Path;

use image::codecs::gif::{GifEncoder, Repeat};
use image::{Delay, Frame, RgbaImage};
use terrain::{compute_normals_vector_par, compute_shadow_vector_par_with_azimuth};

// Lower resolution so the GIF file is manageable
// const GIF_WIDTH: u32 = 800;
// const GIF_HEIGHT: u32 = 267; // 3:1 ratio
const GIF_WIDTH: u32 = 1600;
const GIF_HEIGHT: u32 = 533;
const N_FRAMES: usize = 60;
const FOV_DEG: f32 = 100.0;
const STEP_SIZE: f32 = 1.0;
const T_MAX: f32 = 200_000.0;
const SUN_DIR: [f32; 3] = [0.4, 0.5, 0.7];

// Camera pan: move 100m east per frame
const PAN_PER_FRAME: f32 = 100.0;
const CAM_COL: f32 = 2457.0;
const CAM_ROW: f32 = 3328.0;
const CAM_Z: f32 = 3341.0;
const LAT_X: f32 = 19_627.0;
const LAT_Y: f32 = -1_718.0;
const LAT_Z: f32 = -131.0;

pub(crate) fn render_gif(tile_path: &Path) {
    println!(
        "---------- Rendering GIF {}x{} × {} frames ----------",
        GIF_WIDTH, GIF_HEIGHT, N_FRAMES
    );

    let heightmap = dem_io::parse_bil(tile_path).unwrap();
    let dx = heightmap.dx_meters as f32;
    let dy = heightmap.dy_meters as f32;
    let step_m = heightmap.dx_meters as f32 / STEP_SIZE;
    let aspect = GIF_WIDTH as f32 / GIF_HEIGHT as f32;

    let sun_azimuth_rad = (SUN_DIR[0]).atan2(-SUN_DIR[1]);
    let sun_elevation_rad = SUN_DIR[2].atan2((SUN_DIR[0].powi(2) + SUN_DIR[1].powi(2)).sqrt());

    let shadow_mask = compute_shadow_vector_par_with_azimuth(
        &heightmap,
        sun_azimuth_rad,
        sun_elevation_rad,
        200.0,
    );
    let normal_map = compute_normals_vector_par(&heightmap);

    // AO compute
    let ao_data_mask: Vec<f32> =
        terrain::compute_ao_true_hemi(&heightmap, 16, 5.0f32.to_radians(), 200.0);

    let scene = render_gpu::GpuScene::new(
        render_gpu::GpuContext::new(),
        &heightmap,
        &normal_map,
        &shadow_mask,
        &ao_data_mask,
        GIF_WIDTH,
        GIF_HEIGHT,
    );

    // Warm up pipeline
    let (o, la) = frame_cam(0, dx, dy);
    let _ = scene.render_frame(o, la, FOV_DEG, aspect, SUN_DIR, step_m, T_MAX, 0);

    let out_path = "artifacts/animation.gif";
    let file = File::create(out_path).expect("cannot create artifacts/animation.gif");
    let mut encoder = GifEncoder::new_with_speed(file, 10); // speed 10 = fastest encode
    encoder.set_repeat(Repeat::Infinite).unwrap();

    let t0 = profiling::now();

    for i in 0..N_FRAMES {
        let (origin, look_at) = frame_cam(i, dx, dy);
        let pixels =
            scene.render_frame(origin, look_at, FOV_DEG, aspect, SUN_DIR, step_m, T_MAX, 0);

        let img =
            RgbaImage::from_raw(GIF_WIDTH, GIF_HEIGHT, pixels).expect("pixel buffer size mismatch");

        // 50ms per frame = 20 fps
        let delay = Delay::from_numer_denom_ms(50, 1);
        encoder
            .encode_frame(Frame::from_parts(img, 0, 0, delay))
            .unwrap();

        if (i + 1) % 10 == 0 {
            let elapsed = (profiling::now() - t0) as f64 / crate::utils::counter_frequency();
            println!("  frame {}/{} — {:.1}s elapsed", i + 1, N_FRAMES, elapsed);
        }
    }

    let total = (profiling::now() - t0) as f64 / crate::utils::counter_frequency();
    println!(
        "GIF saved to {}  ({} frames, {:.1}ms/frame, {:.1}s total)",
        out_path,
        N_FRAMES,
        total / N_FRAMES as f64 * 1000.0,
        total,
    );
}

fn frame_cam(i: usize, dx: f32, dy: f32) -> ([f32; 3], [f32; 3]) {
    let pan = i as f32 * PAN_PER_FRAME;
    let origin = [CAM_COL * dx + pan, CAM_ROW * dy, CAM_Z];
    let look_at = [
        CAM_COL * dx + LAT_X + pan,
        CAM_ROW * dy + LAT_Y,
        CAM_Z + LAT_Z,
    ];
    (origin, look_at)
}
