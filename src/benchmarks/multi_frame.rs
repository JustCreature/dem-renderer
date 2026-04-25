use crate::utils::counter_frequency;

use dem_io::Heightmap;
use terrain::{NormalMap, ShadowMask};

const N_FRAMES: usize = 10;
// Resolution and camera matching the main big-render benchmark
const PIC_WIDTH: u32 = 8000;
const PIC_HEIGHT: u32 = 2667;
// const PIC_WIDTH: u32 = 2000;
// const PIC_HEIGHT: u32 = 900;
const FOV_DEG: f32 = 100.0;
const STEP_SIZE: f32 = 1.0;
const T_MAX: f32 = 200_000.0;
const SUN_DIR: [f32; 3] = [0.4, 0.5, 0.7];

// Same viewpoint as the main benchmark
const CAM_COL: f32 = 2457.0;
const CAM_ROW: f32 = 3328.0;
// look_at offset from camera in metres
const LAT_OFF_X: f32 = 19_627.0;
const LAT_OFF_Y: f32 = -1_718.0;
const LAT_OFF_Z: f32 = -131.0;
const CAM_Z: f32 = 3341.0;

/// Generate camera origin/look_at for frame `i`.
/// Simulates a gentle pan: origin slides 200 m east per frame.
fn frame_cam(i: usize, dx: f32, dy: f32) -> ([f32; 3], [f32; 3]) {
    let pan = i as f32 * 200.0;
    let origin = [CAM_COL * dx + pan, CAM_ROW * dy, CAM_Z];
    let look_at = [
        CAM_COL * dx + LAT_OFF_X + pan,
        CAM_ROW * dy + LAT_OFF_Y,
        CAM_Z + LAT_OFF_Z,
    ];
    (origin, look_at)
}

// ── CPU parallel ──────────────────────────────────────────────────────────────

pub(crate) fn benchmark_multi_frame_cpu(
    heightmap: &Heightmap,
    normal_map: &NormalMap,
    shadow_mask: &ShadowMask,
) {
    let dx = heightmap.dx_meters as f32;
    let dy = heightmap.dy_meters as f32;
    let step_m = heightmap.dx_meters as f32 / STEP_SIZE;
    let aspect = PIC_WIDTH as f32 / PIC_HEIGHT as f32;

    // Warm-up: Metal/CPU JIT, rayon thread-pool spin-up
    {
        let (origin, look_at) = frame_cam(0, dx, dy);
        let cam = render_cpu::Camera::new(origin, look_at, FOV_DEG, aspect);
        let _ = render_cpu::render_par(
            &cam,
            heightmap,
            normal_map,
            shadow_mask,
            SUN_DIR,
            PIC_WIDTH,
            PIC_HEIGHT,
            step_m,
            T_MAX,
        );
    }

    let t0 = profiling::now();
    for i in 0..N_FRAMES {
        let (origin, look_at) = frame_cam(i, dx, dy);
        let cam = render_cpu::Camera::new(origin, look_at, FOV_DEG, aspect);
        let _ = std::hint::black_box(render_cpu::render_par(
            &cam,
            heightmap,
            normal_map,
            shadow_mask,
            SUN_DIR,
            PIC_WIDTH,
            PIC_HEIGHT,
            step_m,
            T_MAX,
        ));
    }
    let total_s = (profiling::now() - t0) as f64 / counter_frequency();
    println!(
        "multi_frame CPU_parallel    {}x{}: {:5.1}ms/frame  ({:.2}s total, {} frames)",
        PIC_WIDTH,
        PIC_HEIGHT,
        total_s / N_FRAMES as f64 * 1000.0,
        total_s,
        N_FRAMES,
    );
}

// ── GPU separate (normals + shadow pre-computed once; render per frame) ───────

pub(crate) fn benchmark_multi_frame_gpu_separate(
    gpu_ctx: &render_gpu::GpuContext,
    heightmap: &Heightmap,
    normal_map: &NormalMap,
    shadow_mask: &ShadowMask,
) {
    let dx = heightmap.dx_meters as f32;
    let dy = heightmap.dy_meters as f32;
    let step_m = heightmap.dx_meters as f32 / STEP_SIZE;
    let aspect = PIC_WIDTH as f32 / PIC_HEIGHT as f32;

    // Warm-up
    {
        let (origin, look_at) = frame_cam(0, dx, dy);
        let _ = render_gpu::render_gpu_texture(
            gpu_ctx,
            origin,
            look_at,
            FOV_DEG,
            aspect,
            heightmap,
            normal_map,
            shadow_mask,
            SUN_DIR,
            PIC_WIDTH,
            PIC_HEIGHT,
            step_m,
            T_MAX,
        );
    }

    let t0 = profiling::now();
    for i in 0..N_FRAMES {
        let (origin, look_at) = frame_cam(i, dx, dy);
        let _ = std::hint::black_box(render_gpu::render_gpu_texture(
            gpu_ctx,
            origin,
            look_at,
            FOV_DEG,
            aspect,
            heightmap,
            normal_map,
            shadow_mask,
            SUN_DIR,
            PIC_WIDTH,
            PIC_HEIGHT,
            step_m,
            T_MAX,
        ));
    }
    let total_s = (profiling::now() - t0) as f64 / counter_frequency();
    println!(
        "multi_frame GPU_separate    {}x{}: {:5.1}ms/frame  ({:.2}s total, {} frames)",
        PIC_WIDTH,
        PIC_HEIGHT,
        total_s / N_FRAMES as f64 * 1000.0,
        total_s,
        N_FRAMES,
    );
}

// ── GPU scene (static data uploaded once; only camera written per frame) ─────

pub(crate) fn benchmark_multi_frame_gpu_scene(
    gpu_ctx: render_gpu::GpuContext,
    heightmap: &Heightmap,
    shadow_mask: &ShadowMask,
) {
    let dx = heightmap.dx_meters as f32;
    let dy = heightmap.dy_meters as f32;
    let step_m = heightmap.dx_meters as f32 / STEP_SIZE;
    let aspect = PIC_WIDTH as f32 / PIC_HEIGHT as f32;

    // new() uploads heightmap + computes normals once
    let scene = render_gpu::GpuScene::new(gpu_ctx, heightmap, shadow_mask, PIC_WIDTH, PIC_HEIGHT);

    // Warm-up: Metal compiles the render pipeline on first dispatch
    {
        let (origin, look_at) = frame_cam(0, dx, dy);
        let _ = scene.render_frame(origin, look_at, FOV_DEG, aspect, SUN_DIR, step_m, T_MAX);
    }

    let t0 = profiling::now();
    for i in 0..N_FRAMES {
        let (origin, look_at) = frame_cam(i, dx, dy);
        let _ = std::hint::black_box(
            scene.render_frame(origin, look_at, FOV_DEG, aspect, SUN_DIR, step_m, T_MAX),
        );
    }
    let total_s = (profiling::now() - t0) as f64 / counter_frequency();
    println!(
        "multi_frame GPU_scene       {}x{}: {:5.1}ms/frame  ({:.2}s total, {} frames)",
        PIC_WIDTH,
        PIC_HEIGHT,
        total_s / N_FRAMES as f64 * 1000.0,
        total_s,
        N_FRAMES,
    );
}

// ── GPU combined (normals recomputed on GPU every frame) ─────────────────────

pub(crate) fn benchmark_multi_frame_gpu_combined(
    gpu_ctx: &render_gpu::GpuContext,
    heightmap: &Heightmap,
    shadow_mask: &ShadowMask,
) {
    let dx = heightmap.dx_meters as f32;
    let dy = heightmap.dy_meters as f32;
    let step_m = heightmap.dx_meters as f32 / STEP_SIZE;
    let aspect = PIC_WIDTH as f32 / PIC_HEIGHT as f32;

    // Warm-up
    {
        let (origin, look_at) = frame_cam(0, dx, dy);
        let _ = render_gpu::render_gpu_combined(
            gpu_ctx,
            heightmap,
            shadow_mask,
            origin,
            look_at,
            FOV_DEG,
            aspect,
            SUN_DIR,
            PIC_WIDTH,
            PIC_HEIGHT,
            step_m,
            T_MAX,
        );
    }

    let t0 = profiling::now();
    for i in 0..N_FRAMES {
        let (origin, look_at) = frame_cam(i, dx, dy);
        let _ = std::hint::black_box(render_gpu::render_gpu_combined(
            gpu_ctx,
            heightmap,
            shadow_mask,
            origin,
            look_at,
            FOV_DEG,
            aspect,
            SUN_DIR,
            PIC_WIDTH,
            PIC_HEIGHT,
            step_m,
            T_MAX,
        ));
    }
    let total_s = (profiling::now() - t0) as f64 / counter_frequency();
    println!(
        "multi_frame GPU_combined    {}x{}: {:5.1}ms/frame  ({:.2}s total, {} frames)",
        PIC_WIDTH,
        PIC_HEIGHT,
        total_s / N_FRAMES as f64 * 1000.0,
        total_s,
        N_FRAMES,
    );
}
