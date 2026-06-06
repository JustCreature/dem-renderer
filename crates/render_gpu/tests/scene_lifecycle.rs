//! GPU lifecycle / bind-group-validity integration tests.
//!
//! These need a real device and skip cleanly when none is available. The oracle
//! is wgpu's validation layer: `GpuScene::new` builds the canonical 20-entry bind
//! group, and any binding type/format/count mismatch makes `submit` + `poll`
//! panic on the calling thread, failing the test. "Submit + drain without a
//! panic" is therefore the assertion.
//!
//! No process-global state is touched here, so these run in parallel (unlike the
//! `#[serial]` vram / oom suites).

mod common;

use common::*;
use render_gpu::GpuScene;

fn dispatch_once(scene: &GpuScene, ctx: &render_gpu::GpuContext) {
    let mut enc = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    scene.dispatch_frame(
        &mut enc,
        [0.0, 0.0, 1000.0], // origin
        [100.0, 100.0, 0.0], // look_at
        60.0,               // fov
        1.0,                // aspect
        [0.3, 0.3, 0.9],    // sun_dir
        10.0,               // step_m
        20000.0,            // t_max
        0,                  // ao_mode
        1,                  // shadows_enabled
        1,                  // fog_enabled
        0,                  // vat_mode
        0,                  // lod_mode
        2000.0,             // smooth_radius_m
        0,                  // align_mode
    );
    ctx.queue.submit([enc.finish()]);
    drain(ctx);
}

#[test]
fn scene_builds_and_dispatch_survives_validation() {
    gpu_or_skip!(ctx);

    let h = pseudo_random(64, 64, 0xA11CE, 500.0, 10.0, 10.0);
    let (n, s, ao) = derive_maps(&h);
    let scene = GpuScene::new(ctx.clone(), &h, &n, &s, &ao, 128, 128);

    // One dispatch against the freshly built 20-entry bind group.
    dispatch_once(&scene, &ctx);
}

#[test]
fn resize_updates_output_buffer() {
    gpu_or_skip!(ctx);

    let h = pseudo_random(48, 48, 7, 300.0, 10.0, 10.0);
    let (n, s, ao) = derive_maps(&h);
    let mut scene = GpuScene::new(ctx.clone(), &h, &n, &s, &ao, 64, 64);

    assert_eq!(scene.get_output_buffer().size(), 64 * 64 * 4);
    dispatch_once(&scene, &ctx);

    scene.resize(96, 80);
    assert_eq!(scene.get_output_buffer().size(), 96 * 80 * 4);
    // Dispatch again at the new size — rebuilt bind group must still validate.
    dispatch_once(&scene, &ctx);
}

#[test]
fn tier_upload_then_inactive_survives_validation() {
    gpu_or_skip!(ctx);

    let base = pseudo_random(64, 64, 1, 500.0, 30.0, 30.0);
    let (bn, bs, bao) = derive_maps(&base);
    let mut scene = GpuScene::new(ctx.clone(), &base, &bn, &bs, &bao, 96, 96);
    dispatch_once(&scene, &ctx);

    // Upload close (5 m) and fine (1 m) tiers — exercises rebuild_bind_group on
    // the grow path (bindings 10–19).
    let close = pseudo_random(80, 80, 2, 400.0, 5.0, 5.0);
    let (cn, cs, _) = derive_maps(&close);
    let close_norm = render_gpu::pack_normals_rg16_bytes(&cn.nx, &cn.ny);
    scene.upload_hm5m(0.0, 0.0, 0.0, 400.0, 400.0, &close, &close_norm, &cs);
    dispatch_once(&scene, &ctx);

    let fine = pseudo_random(72, 72, 3, 200.0, 1.0, 1.0);
    let (fn_, fs, _) = derive_maps(&fine);
    let fine_norm = render_gpu::pack_normals_rg16_bytes(&fn_.nx, &fn_.ny);
    scene.upload_hm1m(0.0, 0.0, 0.0, 72.0, 72.0, &fine, &fine_norm, &fs);
    dispatch_once(&scene, &ctx);

    // Deactivate both tiers (drop-first placeholder path) and dispatch again.
    scene.set_hm1m_inactive();
    scene.set_hm5m_inactive();
    dispatch_once(&scene, &ctx);
}
