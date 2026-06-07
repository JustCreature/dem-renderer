//! Shared fixtures for the `render_gpu` integration tests.
//!
//! These tests need a real `wgpu::Device`. On a machine with no usable adapter
//! (headless CI) they must skip cleanly rather than fail — see [`try_ctx`] and
//! the [`gpu_or_skip`] macro, which mirror the avx2 "not available, skipping"
//! idiom in `crates/terrain/tests/shadow.rs`.

#![allow(dead_code)] // each test binary uses a different subset of these helpers

use dem_io::Heightmap;
use render_gpu::{GpuContext, VramClass};
use terrain::{NormalMap, ShadowMask};

/// Try to build a `GpuContext` without panicking when no adapter exists.
///
/// We do **not** call `GpuContext::new` (it `.expect()`s an adapter and panics
/// headless). Instead we request a fallible adapter and build the struct by
/// hand — all its fields are `pub`. The device request mirrors
/// `context.rs`: full adapter limits, and the optional precision features masked
/// by what the adapter actually supports.
pub fn try_ctx() -> Option<GpuContext> {
    pollster::block_on(async {
        let instance = wgpu::Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                ..Default::default()
            })
            .await
            .ok()?;

        let info = adapter.get_info();
        let vram_class = VramClass::detect(&info);

        let wanted = wgpu::Features::FLOAT32_FILTERABLE | wgpu::Features::TEXTURE_FORMAT_16BIT_NORM;
        let enabled = adapter.features() & wanted;

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_features: enabled,
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await
            .ok()?;

        // We deliberately keep wgpu's default uncaptured-error handler, which
        // panics on the calling thread. A bind-group / dispatch validation
        // mismatch therefore fails the test that triggered it — "no panic after
        // submit + poll" is the lifecycle assertion. No global error flag is
        // needed, so the lifecycle tests run in parallel.

        Some(GpuContext {
            instance,
            device,
            queue,
            adapter_name: info.name,
            adapter,
            vram_class,
        })
    })
}

/// Bind `$name` to a `GpuContext`, or `eprintln!` + `return` from the test when
/// no adapter is available.
#[macro_export]
macro_rules! gpu_or_skip {
    ($name:ident) => {
        let $name = match $crate::common::try_ctx() {
            Some(c) => c,
            None => {
                eprintln!("no GPU adapter — skipping");
                return;
            }
        };
    };
}

/// Block until the queue drains, so any deferred validation error has been
/// delivered (and panicked) before the test returns.
pub fn drain(ctx: &GpuContext) {
    let _ = ctx.device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
}

// ── heightmap / map builders ─────────────────────────────────────────────────

/// Build a `Heightmap` from raw row-major data + cell size. Geo/CRS fields are
/// neutral placeholders (mirrors `crates/terrain/tests/common/mod.rs`).
pub fn hm(rows: usize, cols: usize, data: Vec<f32>, dx_meters: f64, dy_meters: f64) -> Heightmap {
    assert_eq!(data.len(), rows * cols, "data length must equal rows*cols");
    Heightmap {
        data,
        rows,
        cols,
        nodata: -9999.0,
        origin_lat: 0.0,
        origin_lon: 0.0,
        dx_deg: 0.0,
        dy_deg: 0.0,
        dx_meters,
        dy_meters,
        crs_origin_x: 0.0,
        crs_origin_y: 0.0,
        crs_epsg: 0,
        crs_proj4: String::new(),
    }
}

/// Deterministic pseudo-random terrain (xorshift64), values in `[0, amplitude)`.
pub fn pseudo_random(
    rows: usize,
    cols: usize,
    seed: u64,
    amplitude: f32,
    dx: f64,
    dy: f64,
) -> Heightmap {
    let mut state = seed | 1;
    let mut data = vec![0.0f32; rows * cols];
    for v in data.iter_mut() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let unit = (state >> 11) as f32 / (1u64 << 53) as f32;
        *v = unit * amplitude;
    }
    hm(rows, cols, data, dx, dy)
}

/// Real terrain-kernel outputs for a heightmap, so the GPU upload sees valid
/// (not zeroed) data. Returns `(normals, shadow, ao)`.
pub fn derive_maps(h: &Heightmap) -> (NormalMap, ShadowMask, Vec<f32>) {
    let normals = terrain::compute_normals_vector(h);
    let shadow = terrain::compute_shadow_vector(h, 0.5);
    let ao = vec![1.0f32; h.rows * h.cols];
    (normals, shadow, ao)
}
