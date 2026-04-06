# Phase 5 Session Log

## Overview

Phase 5 implemented the GPU renderer using wgpu compute shaders, benchmarked it against the CPU renderer from Phase 4, and built a persistent `GpuScene` pattern for animation. The phase concluded with a full benchmark run and updated reports.

---

## Session 1 — GPU Renderer Foundation

### What was built

Started from scratch with wgpu boilerplate: `Instance → Adapter → Device → Queue` initialization chain, `wgpu::Buffer` allocations for heightmap / normals / shadow / camera / output.

First working shader was `shader_buffer.wgsl` — heightmap accessed as a flat `array<f32>` storage buffer. Each GPU thread marches one ray. Workgroup size 8×8 = 64 threads.

**First errors encountered:**
- All-black output: heightmap was uploaded as raw `i16` bytes without converting to `f32` first. The shader reads `f32` values, so every elevation came out as garbage.
- Banded stripes: used `RgbImage::from_raw` (3 bytes/pixel) with an RGBA output buffer (4 bytes/pixel). Fix: `RgbaImage::from_raw`.
- Wrong terrain shape: `pos.y / dx_meters` instead of `dy_meters` — cell sizes are not square (at 47°N: dx ≈ 21m, dy ≈ 31m).
- Arc artifacts in foreground: all rays hitting at step N land at exactly `N * step_m`, creating concentric rings. Fix: binary search between step N-1 and step N, 8 iterations.
- Uniform buffer size mismatch: `vec3<f32>` in WGSL occupies 16 bytes (12 data + 4 pad). Every `[f32; 3]` in the Rust `CameraUniforms` struct needs an explicit `_pad: f32` after it. Also the struct total must be a multiple of 16 bytes.

### CameraUniforms layout

```rust
#[repr(C)]
struct CameraUniforms {
    origin:  [f32;3], _pad0: f32,   // 16 bytes
    forward: [f32;3], _pad1: f32,   // 16 bytes
    right:   [f32;3], _pad2: f32,   // 16 bytes
    up:      [f32;3], _pad3: f32,   // 16 bytes
    sun_dir: [f32;3], _pad4: f32,   // 16 bytes
    half_w: f32, half_h: f32, img_width: u32, img_height: u32,  // 16 bytes
    hm_cols: u32, hm_rows: u32, dx_meters: f32, dy_meters: f32, // 16 bytes
    step_m: f32, t_max: f32, _pad5: f32, _pad6: f32,            // 16 bytes
}  // = 128 bytes
```

### Texture variant

Built `shader_texture.wgsl` and `render_rexture.rs` (typo in filename preserved). Heightmap uploaded as a `texture_2d<f32>` with `R32Float` format. Shader uses `textureLoad(hm_tex, vec2<i32>(col, row), 0).r`.

`textureLoad` requires a sampler binding even if no sampling is done (wgpu validation). Used `SamplerBindingType::NonFiltering`.

Hypothesis: texture cache (2D Morton-order internally) should outperform linear buffer for 2D spatial access.

---

## Session 2 — GpuContext, Combined Renderer, Benchmarks

### The init cost problem

First benchmark: GPU render at 8000×2667. Every call to `render_gpu_buffer` re-ran `Instance → Adapter → Device → Queue` — ~80ms per call, before any compute. For 10-frame animation = 800ms of pure init.

### GpuContext

Extracted the init chain into `context.rs`:

```rust
pub struct GpuContext { pub device: wgpu::Device, pub queue: wgpu::Queue }
```

All render functions refactored to take `&GpuContext` as first parameter. Init runs once in `main()`, all calls share the same device.

### render_gpu_combined

`render_gpu_combined` computes normals on the GPU inside the render pass — no `NormalMap` parameter, no 156 MB upload. Two pipeline dispatches per call: first normals (`shader_normals.wgsl`), then render (`shader_buffer.wgsl`). This saved ~40–80ms/call vs `render_gpu_texture` at 8000×2667.

### Multi-frame benchmark

Implemented `src/benchmarks/multi_frame.rs` with 4 variants:
- `benchmark_multi_frame_cpu`: 10 frames of `render_cpu::render_par` — 1730 ms/frame
- `benchmark_multi_frame_gpu_separate`: `render_gpu_texture` — 133 ms/frame
- `benchmark_multi_frame_gpu_combined`: `render_gpu_combined` — 120 ms/frame
- `benchmark_multi_frame_gpu_scene`: `GpuScene::render_frame` — 98 ms/frame

Each variant includes a warmup call before timing begins.

---

## Session 3 — GpuScene: Persistent GPU Resources

### The problem

Both `render_gpu_texture` and `render_gpu_combined` re-upload all data on every call. For animation:
- `render_gpu_texture`: heightmap (52 MB) + normals (156 MB) + shadow (13 MB) = 221 MB/frame
- `render_gpu_combined`: heightmap + shadow = 65 MB/frame + GPU normals recompute

The terrain doesn't change. Only the camera moves. 128 bytes per frame.

### GpuScene design

`GpuScene::new(ctx: GpuContext, hm, shadow_mask, width, height)`:

1. Upload heightmap as texture once (52 MB)
2. Allocate nx/ny/nz STORAGE buffers (156 MB GPU memory) — never filled from CPU
3. Run normals compute pass on GPU: dispatch `shader_normals.wgsl`, poll to completion, then drop the normals pipeline and bind group — only the filled output buffers remain
4. Upload shadow (13 MB)
5. Allocate `cam_buf` (UNIFORM | COPY_DST, 128 bytes)
6. Allocate `output_buf` (STORAGE | COPY_SRC, 85 MB for 8000×2667 RGBA) and `readback_buf` (MAP_READ | COPY_DST)
7. Create render pipeline once (Metal driver compiles the shader)
8. Create render bind group once

`render_frame()` per-call cost:
- `queue.write_buffer(&cam_buf, 0, bytemuck::bytes_of(&uniforms))` — 128 bytes
- `encoder.begin_compute_pass` → dispatch → `encoder.copy_buffer_to_buffer`
- `queue.submit` → `device.poll(Wait)` → `map_async` → copy pixels

### wgpu bind group lifetime lesson

Discovered: wgpu bind groups store GPU buffer addresses, not CPU-side Arc refs. If a buffer goes out of scope, the GPU address is freed even though the bind group still references it. The next dispatch would access freed memory.

Fix: all resources referenced by a bind group must be stored as owned fields in the same struct. Fields only needed for lifetime (never accessed directly) use `_` prefix: `_hm_texture`, `_nx_buf`, etc.

### update_shadow()

`update_shadow(&self, shadow_mask: &ShadowMask)` — writes 13 MB into the existing `shadow_buf` via `write_buffer`. The bind group automatically sees new data on next dispatch. Used for sun animation where the shadow changes each frame.

Shadow is CPU-computed (NEON parallel) because the running-max dependency chain is serial per row — GPU shader cores have no advantage over CPU at lower clock speed for serial work.

### GIF renderer

`src/render_gif.rs` — 60 frames at 1600×533 (3:1 aspect), 20fps, using `GpuScene`. Sun direction fixed; camera pans 100m east per frame. Each frame: `scene.render_frame()` → `RgbaImage` → `GifEncoder::encode_frame` with 50ms delay. Output: `artifacts/animation.gif`.

---

## Session 4 — Full Benchmark Run and Reports

### Discussion: GPU shadow and RTX

User asked whether GPU shadow computation and RTX ray tracing make sense for sun animation.

Key points delivered:
- Shadow's serial running-max can be parallelised across rows on GPU (each row independent) → parallel prefix scan approach, O(log N) passes
- For sun animation, GPU parallel sweep would beat CPU NEON (~1.5ms → potentially sub-ms)
- RTX hardware (BVH + ray-triangle intersection) is wrong for heightfields: the grid structure is the acceleration structure; DDA/min-max mipmaps exploit it natively
- Apple Silicon has no RT cores; RTX is NVIDIA Turing+ only
- The right GPU shadow approach: parallel prefix max with workgroup shared memory, standard GPU primitive

### Full benchmark run

Uncommented `benchmark_multi_frame_cpu` (previously commented out), commented out `render_gif` call. Built release and ran. Full output captured to `docs/sessions/phase-5/benchmark_results.md`.

Key numbers:
- GPU scene: 98 ms/frame vs CPU 1730 ms/frame — 17.7×
- Buffer (130ms) beats texture (170ms) at single-shot — contradicts naive texture-cache hypothesis
- Shadow: CPU NEON parallel 1.5ms vs GPU 26ms — CPU wins 17×
- Diagonal shadow (any azimuth) 2.4× slower than cardinal due to strided cache-line access

### Texture vs buffer flip

Previous session had shown texture winning (0.11s vs 0.13s for buffer). Current run shows buffer winning (0.13s vs 0.17s). Both numbers consistent at multi-frame steady state (~133ms). Analysis: raymarching's stripe-like access doesn't benefit 2D Morton-order texture cache layout; sampler unit adds ~20–40 cycle latency for no gain when `textureLoad` (integer coords, no interpolation) is used.

### Reports updated

Both `docs/lessons/phase-5/long-report.md` and `short-report.md` updated with:
- All fresh numbers from 2026-04-06 run
- New sections: GpuContext, GpuScene, multi-frame benchmark analysis
- Corrected texture vs buffer conclusion
- Bind group lifetime lesson added
- Readback floor analysis added

---

## Final Numbers (M4 Max, 2026-04-06)

### Single-image render 8000×2667
| Variant | Time | vs CPU |
|---|---|---|
| CPU parallel 10 cores | 1.26 s | 1× |
| GPU buffer | 130 ms | 9.7× |
| GPU texture | 170 ms | 7.4× |
| GPU combined | 90 ms | 14.0× |

### Multi-frame steady-state 8000×2667
| Variant | ms/frame | vs CPU |
|---|---|---|
| CPU parallel | 1730.8 | 1× |
| GPU separate | 133.4 | 13.0× |
| GPU combined | 120.4 | 14.4× |
| GPU scene | **98.0** | **17.7×** |

### Shadow (3601×3601)
| Variant | Throughput / Time |
|---|---|
| NEON parallel cardinal | 55.4 GB/s |
| NEON parallel diagonal | 26.3 GB/s |
| GPU | 26 ms (CPU wins ~17×) |

---

## Session 5 — Workgroup Size Experiment (2026-04-06)

### Open item closed

Benchmarked all planned workgroup size variants to close the "Workgroup size not benchmarked" open item.

### Method

Added `render_gpu_buffer_wg(wx, wy)` to `render_buffer.rs` — patches `@workgroup_size(8, 8, 1)` via string replacement at shader compile time, adjusts dispatch to ceiling division. Code was reverted after measurement; results captured below.

### Results (8000×2667, single render after per-variant warmup)

| Workgroup | Threads | SIMD groups | Time |
|---|---|---|---|
| 8×8 | 64 | 2 | 136.1 ms |
| 16×4 | 64 | 2 | 138.7 ms |
| 4×16 | 64 | 2 | 136.1 ms |
| 32×2 | 64 | 2 | 140.2 ms |
| 16×16 | 256 | 8 | 137.5 ms |
| 32×8 | 256 | 8 | 138.1 ms |
| 8×32 | 256 | 8 | 135.6 ms |

All variants within ±3%. Workgroup size has no measurable effect. Bottleneck is the 85 MB readback (~88 ms); the compute dispatch itself is ~5–10 ms, too short for occupancy tuning to show up in wall time.

### Occupancy analysis status

Full Instruments/Metal GPU trace requires Xcode.app (~15 GB). Not installed. Deferred — not a priority given that readback dominates and workgroup tuning shows no effect.

### Reports updated

`docs/lessons/phase-5/long-report.md` and `short-report.md` updated with Part 12 / Section 12 covering the workgroup experiment.
