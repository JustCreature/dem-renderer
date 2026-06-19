//! Coverage for the public `GpuContext` constructors (`new` / `new_async` / `default`).
//!
//! The other GPU suites build a context by hand (`common::try_ctx`) to stay headless-safe,
//! so the real `GpuContext::new` device-creation path was never exercised. This file calls
//! it directly — but only after `try_ctx` confirms an adapter exists, so it still skips
//! cleanly on a headless runner and never hits the `.expect("no GPU adapter found")` panic.

mod common;

use render_gpu::{GpuContext, VramClass};

#[test]
fn new_builds_a_usable_context() {
    // Probe for an adapter; skip headless. If this succeeds, `GpuContext::new` (which
    // `.expect()`s an adapter) is guaranteed not to panic on the same default instance.
    gpu_or_skip!(_probe);

    let ctx = GpuContext::new();

    assert!(
        !ctx.adapter_name.is_empty(),
        "adapter_name should be populated"
    );
    assert!(
        matches!(
            ctx.vram_class,
            VramClass::Low | VramClass::Mid | VramClass::High
        ),
        "vram_class must be a detected variant"
    );

    // The device is live: a trivial poll on a fresh context must not error.
    let res = ctx.device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    assert!(res.is_ok(), "device.poll on a fresh context should succeed");

    // Clone is a cheap Arc bump that shares the same device (documented invariant).
    let ctx2 = ctx.clone();
    assert_eq!(ctx2.adapter_name, ctx.adapter_name);
}

#[test]
fn new_async_builds_a_usable_context() {
    gpu_or_skip!(_probe);

    // `new` is just `block_on(new_async())`; awaiting the async form directly covers it
    // without the blocking wrapper (and is the entry point the wasm build uses).
    let ctx = pollster::block_on(GpuContext::new_async());
    assert!(!ctx.adapter_name.is_empty());
}

#[test]
fn default_constructs_via_new() {
    gpu_or_skip!(_probe);

    let ctx = GpuContext::default();
    assert!(!ctx.adapter_name.is_empty());
}
