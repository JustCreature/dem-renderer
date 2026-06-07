//! Drop-first VRAM-accounting integration tests.
//!
//! These validate the crate's marquee design decision: a tier reload's peak GPU
//! memory is `max(old, new)`, not `old + new`, because the old resources are
//! dropped (and the BindGroup rebuilt) *before* the new ones are allocated. The
//! only observable signal is `vram::GPU_TEXTURE_BYTES`, the process-global
//! allocation tracker — so every test here is `#[serial]` and asserts on
//! *deltas* captured at its own start (the counter accumulates across tests and
//! is never decremented on scene drop).
//!
//! Skips cleanly when no GPU adapter is available.

mod common;

use std::sync::atomic::Ordering;

use common::*;
use render_gpu::GpuScene;
use render_gpu::vram::GPU_TEXTURE_BYTES;
use serial_test::serial;

fn tex_bytes() -> i64 {
    GPU_TEXTURE_BYTES.load(Ordering::Relaxed) as i64
}

#[test]
#[serial]
fn reload_grow_does_not_double_peak() {
    gpu_or_skip!(ctx);

    let base = pseudo_random(64, 64, 1, 500.0, 30.0, 30.0);
    let (bn, bs, bao) = derive_maps(&base);
    let mut scene = GpuScene::new(ctx.clone(), &base, &bn, &bs, &bao, 64, 64);

    let baseline = tex_bytes();

    // First close-tier upload at 64×64 (placeholder → real). Detail tiers store
    // two 4-bpp textures (R32Float heightmap + Rg16Snorm normals); no mips.
    let small = pseudo_random(64, 64, 2, 400.0, 5.0, 5.0);
    let (sn, ss, _) = derive_maps(&small);
    let small_norm = render_gpu::pack_normals_rg16_bytes(&sn.nx, &sn.ny);
    scene.upload_hm5m(0.0, 0.0, 0.0, 320.0, 320.0, &small, &small_norm, &ss);
    let small_foot = tex_bytes() - baseline;
    assert!(small_foot > 0, "first upload should grow tracked textures");

    // Re-upload the SAME size → grow-only logic writes in place, allocates
    // nothing. Tracked bytes must be unchanged.
    let small2 = pseudo_random(64, 64, 9, 400.0, 5.0, 5.0);
    let (sn2, ss2, _) = derive_maps(&small2);
    let small2_norm = render_gpu::pack_normals_rg16_bytes(&sn2.nx, &sn2.ny);
    scene.upload_hm5m(0.0, 0.0, 0.0, 320.0, 320.0, &small2, &small2_norm, &ss2);
    assert_eq!(
        tex_bytes() - baseline,
        small_foot,
        "same-size reload must not allocate"
    );

    // Grow to 128×128 (4× the area → 4× the tier textures). Drop-first means the
    // old 64×64 tier is released first, so the new total ≈ baseline + 4×small.
    // The sum-bug (no drop) would leave baseline + small + 4×small = 5×small.
    let big = pseudo_random(128, 128, 3, 400.0, 5.0, 5.0);
    let (gn, gs, _) = derive_maps(&big);
    let big_norm = render_gpu::pack_normals_rg16_bytes(&gn.nx, &gn.ny);
    scene.upload_hm5m(0.0, 0.0, 0.0, 640.0, 640.0, &big, &big_norm, &gs);
    let big_foot = tex_bytes() - baseline;

    let ratio = big_foot as f64 / small_foot as f64;
    assert!(
        ratio < 4.5,
        "drop-first expected ~4× (got {ratio:.3}×); ~5× would mean the old \
         tier was not freed before allocating the new one"
    );
    assert!(
        ratio > 3.5,
        "128² tier should be ~4× the 64² tier (got {ratio:.3}×)"
    );
}

#[test]
#[serial]
fn set_inactive_frees_tracked_textures() {
    gpu_or_skip!(ctx);

    let base = pseudo_random(64, 64, 1, 500.0, 30.0, 30.0);
    let (bn, bs, bao) = derive_maps(&base);
    let mut scene = GpuScene::new(ctx.clone(), &base, &bn, &bs, &bao, 64, 64);

    let baseline = tex_bytes();

    let close = pseudo_random(96, 96, 5, 400.0, 5.0, 5.0);
    let (cn, cs, _) = derive_maps(&close);
    let close_norm = render_gpu::pack_normals_rg16_bytes(&cn.nx, &cn.ny);
    scene.upload_hm5m(0.0, 0.0, 0.0, 480.0, 480.0, &close, &close_norm, &cs);
    assert!(
        tex_bytes() - baseline > 0,
        "upload should grow tracked textures"
    );

    // Deactivation runs the same drop-first cycle down to a 1×1 placeholder, so
    // tracked textures return to ~baseline (within the few bytes of the new
    // placeholder vs. the original).
    scene.set_hm5m_inactive();
    let after = tex_bytes() - baseline;
    assert!(
        after.abs() < 1024,
        "inactive tier should free its textures (residual {after} bytes)"
    );
}
