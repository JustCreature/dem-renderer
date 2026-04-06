# Phase 5 — GPU Renderer: Reference Document

**Hardware**: Apple M4 Max (10 cores, 40 GPU shader arrays, 400 GB/s unified memory)
**Dataset**: USGS SRTM N47E011, 3601×3601, ~52 MB as f32
**Date**: 2026-04-06

---

## 1. GPU Execution Model

**SIMT (Single Instruction, Multiple Threads)**: all threads in a SIMD group execute the same instruction simultaneously on different data. Apple Silicon SIMD group = 32 threads. NVIDIA warp = 32. AMD wavefront = 64.

**Workgroup**: a group of threads sharing scratchpad memory. We used 8×8 = 64 threads = 2 SIMD groups. The workgroup is the unit of scheduling on a GPU core.

**Latency hiding**: GPU hides memory latency (400–800 cycles) by switching to another ready SIMD group when one stalls. Requires many SIMD groups in flight (occupancy).

**Divergence**: threads in the same SIMD group taking different branches execute both paths sequentially with inactive lanes masked. Rays hitting at different steps → divergence → partially wasted compute.

---

## 2. wgpu Initialization Chain

```
Instance → Adapter → Device + Queue
```

- `Instance::default()`: detects backend (Metal/Vulkan/D3D12)
- `request_adapter(HighPerformance)`: selects physical GPU
- `request_device()`: creates logical device + submission queue
- Wrapped in `pollster::block_on(async { ... })` to block sync context

**Fixed initialization overhead: ~80ms.** Dominates at small images (≤3–5 Mpix). At 2000×900 (1.8 Mpix), GPU ≈ CPU parallel — entirely init overhead.

---

## 3. GpuContext — Amortize Init Cost

```rust
pub struct GpuContext { pub device: wgpu::Device, pub queue: wgpu::Queue }
```

Create once; pass `&GpuContext` to all render calls. Eliminates the ~80ms init cost from every frame. Same pattern as a database connection pool: expensive connection established once, shared across queries.

---

## 4. Buffer Types and Usages

| Usage | Purpose |
|---|---|
| `STORAGE \| COPY_SRC` | GPU output buffer (shader writes, then copies out) |
| `MAP_READ \| COPY_DST` | Readback buffer (CPU reads after GPU copy) |
| `UNIFORM \| COPY_DST` | Camera parameters (small, cached, read-only in shader) |
| `STORAGE` (read-only) | Heightmap, normals, shadow mask |

Cannot map a `STORAGE` buffer for CPU reading. Must `copy_buffer_to_buffer` to a `MAP_READ` buffer first.

`create_buffer_init`: creates + uploads data in one call. `create_buffer`: creates empty buffer.

---

## 5. Uniform Buffer Layout (std140)

- `f32`/`u32`/`i32`: 4 bytes, 4-byte aligned
- `vec3<f32>`: **16 bytes** (12 data + 4 implicit pad) ← critical rule
- Struct total size must be multiple of 16 bytes

Rust side: `#[repr(C)]` (preserves field order), `bytemuck::Pod` + `bytemuck::Zeroable` (safe cast to `&[u8]`). Add explicit `_pad: f32` fields after every `[f32; 3]`.

`CameraUniforms`: 128 bytes (8 × 16). Fields: origin, forward, right, up, sun_dir (each `[f32;3]` + pad), then half_w, half_h, img_width, img_height, hm_cols, hm_rows, dx_meters, dy_meters, step_m, t_max, 2 pads.

Debugging: wgpu error reports exact expected byte count. Count Rust struct fields, add pads to match.

---

## 6. Bind Group Model

Two objects must match each other and the shader:
1. **BindGroupLayout**: declares resource types at each binding number (schema)
2. **BindGroup**: assigns real resources to those numbers (instance)

`@group(0) @binding(N)` in shader ↔ layout entry with `binding: N` ↔ bind group entry with `binding: N`. All three must agree.

**Critical**: wgpu bind groups do NOT hold CPU-side `Arc` refs to resources. If the owning `wgpu::Buffer` is dropped, the GPU address is freed — the bind group references freed memory. All resources referenced by a bind group must outlive it. Use `_` prefix fields in owning structs to signal "kept alive only, not accessed directly."

**`write_buffer` updates contents in-place**: the buffer's GPU address stays the same. The bound bind group automatically sees new data on the next dispatch — no need to rebuild.

---

## 7. GpuScene — Persistent GPU State

**Problem**: `render_gpu_texture` / `render_gpu_combined` re-upload 221 MB per frame (heightmap + normals + shadow). For animation this dominates render cost.

**Solution**: `GpuScene::new()` uploads everything once:

```
new():
  upload heightmap texture        ← 52 MB, once
  allocate nx/ny/nz STORAGE bufs  ← 156 MB, once
  dispatch normals compute pass   ← GPU fills nx/ny/nz from heightmap
  upload shadow                   ← 13 MB, once
  allocate cam_buf (UNIFORM)      ← 128 bytes, updated per frame
  allocate output_buf + readback  ← 85 MB, once
  create pipeline + bind group    ← compiled/cached once

render_frame():
  write_buffer(cam_buf, 128 bytes)   ← ONLY data transfer per frame
  encode { dispatch; copy to readback }
  submit → poll → map → copy pixels
```

Normals computed on GPU inside a scoped block in `new()`. The pipeline is dropped after one dispatch. Only `_nx_buf`, `_ny_buf`, `_nz_buf` survive — they stay on GPU permanently, never read back to CPU.

`update_shadow(shadow_mask)`: re-uploads 13 MB for sun animation. Caller computes new `ShadowMask` on CPU (NEON parallel), passes it in.

---

## 8. WGSL Shader Structure

```wgsl
@group(0) @binding(0) var<uniform> cam: CameraUniforms;
@group(0) @binding(1) var<storage, read> hm: array<f32>;   // buffer variant
// OR
@group(0) @binding(1) var hm_tex: texture_2d<f32>;         // texture variant
@group(0) @binding(2) var hm_sampler: sampler;

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) { ... }
```

**Dispatch**: `pass.dispatch_workgroups((width+7)/8, (height+7)/8, 1)` — ceiling division.

**Ray direction**:
```wgsl
let u = (f32(gid.x) + 0.5) / f32(cam.img_width);
let ndc_x = -(2.0 * u - 1.0);   // flip horizontal
let ndc_y = 1.0 - 2.0 * v;       // flip vertical
let dir = normalize(cam.forward + cam.right * ndc_x * cam.half_w + cam.up * ndc_y * cam.half_h);
```

**Heightmap read (buffer)**: `hm[u32(row) * cam.hm_cols + u32(col)]`
**Heightmap read (texture)**: `textureLoad(hm_tex, vec2<i32>(col, row), 0).r`

`textureLoad`: integer coords, no interpolation. Returns `vec4<f32>` — use `.r` for single channel.

Binary search refinement between step N-1 and N: 8 iterations → sub-meter precision.

---

## 9. Textures vs Storage Buffers

**Texture cache**: dedicated 2D-aware L1 (~4–16 KB per shader array). Internally Morton-order tiled — a 4×4 texel neighbourhood fits in one cache line.

**Storage buffer cache**: linear. 2D-adjacent rows are `cols * 4 ≈ 14 KB` apart in address space.

**Hypothesis**: texture cache should win because rays in the same workgroup start near each other → nearby terrain → 2D cache hits.

**Measured result (8000×2667, M4 Max)**:

| Variant | Single-shot | Steady-state (multi-frame) |
|---|---|---|
| GPU buffer | **130 ms** | ~130 ms |
| GPU texture | 170 ms | ~133 ms |

**Buffer wins single-shot (1.3×); roughly equal at steady-state.**

Why: raymarching has 1D stripe-like access, not 2D patch access. Rays diverge after a few steps — the texture cache's 2D locality only helps during early traversal. The sampler unit adds latency (~20–40 cycles) on every access even when bilinear interpolation is not needed. Storage buffer reads go directly to L1 without the sampler detour.

---

## 10. GPU vs CPU Performance

At 8000×2667 = 21.3 Mpix, step_m = dx/1.0:

| Renderer | Time | Mpix/s | Speedup |
|---|---|---|---|
| CPU parallel (10 cores) | 1.26 s | 17.0 | 1× |
| GPU buffer | 130 ms | 164 | 9.7× |
| GPU texture | 170 ms | 125 | 7.4× |
| GPU combined | **90 ms** | **237** | **14.0×** |

**Crossover**: at 2000×900 (1.8 Mpix) GPU ≈ CPU parallel (~80ms each). wgpu init overhead dominates below ~3–5 Mpix. Use `GpuContext` to amortize.

**GPU wins rendering because**: embarrassingly parallel; 40 GPU shader arrays × 8 SIMD groups × 32 threads ≈ 10,000 concurrent threads vs 10 CPU threads.

**CPU wins shadow because**: running-max dependency chain is serial per row; GPU shader cores run at lower clock than CPU performance cores; NEON parallel across rows is the ceiling.

---

## 11. Multi-Frame Benchmark (10 frames, 8000×2667)

Camera pans 200m east per frame. Warmup call excluded from timing.

| Variant | ms/frame | vs CPU | vs GPU separate |
|---|---|---|---|
| CPU parallel | 1730.8 | 1× | — |
| GPU separate | 133.4 | 13.0× | 1× |
| GPU combined | 120.4 | 14.4× | 1.11× |
| GPU scene | **98.0** | **17.7×** | **1.36×** |

**Per-frame costs breakdown**:

| Variant | Data upload | Compute | Readback | Overhead |
|---|---|---|---|---|
| GPU separate | ~80 ms (221 MB) | — | ~40 ms | ~13 ms |
| GPU combined | ~30 ms (65 MB) | ~40 ms (normals) | ~40 ms | ~10 ms |
| GPU scene | ~0 ms (128 B) | — | ~88 ms | ~10 ms |

**Readback floor**: 85 MB RGBA at 400 GB/s = 0.2 ms theoretical; ~88 ms empirical (page faults + driver sync). This is the hard floor for the scene pipeline.

---

## 12. Workgroup Size Experiment (M4 Max, 8000×2667, 2026-04-06)

All variants timed after one warm-up call (pipeline compile excluded).

| Workgroup | Threads | SIMD groups | Time |
|---|---|---|---|
| 8×8 | 64 | 2 | 136.1 ms |
| 16×4 | 64 | 2 | 138.7 ms |
| 4×16 | 64 | 2 | 136.1 ms |
| 32×2 | 64 | 2 | 140.2 ms |
| 16×16 | 256 | 8 | 137.5 ms |
| 32×8 | 256 | 8 | 138.1 ms |
| 8×32 | 256 | 8 | 135.6 ms |

**All within ±3% — workgroup size has no measurable effect.**

Why: the bottleneck is the 85 MB RGBA readback (~88 ms), not GPU compute. The dispatch itself takes ~5–10 ms; occupancy gains from larger workgroups are buried under readback time. Shape doesn't matter either — rays diverge immediately, so no sustained 2D locality for any shape to exploit.

To see workgroup size effects: eliminate the readback (swap-chain/display architecture), making the ~5–10 ms compute phase the measurement target.

---

## 13. Full Benchmark Table (M4 Max, 2026-04-06)

### Memory Bandwidth (256 MB buffer)
| Pattern | Throughput |
|---|---|
| seq_read SIMD | 65.8 GB/s |
| seq_read scalar | 5.0 GB/s |
| seq_write | 6.1 GB/s |
| random_read SIMD | 1.4 GB/s |
| random_read scalar | 0.6 GB/s |
| random_write | 0.5 GB/s |

### Tiling / Neighbour-Access (3601×3601)
| Layout | Throughput |
|---|---|
| row-major iteration | 45.2 GB/s |
| tiled, row-major iteration | 4.3 GB/s |
| tiled, tile-order walk | 3.1 GB/s |

### Normal Map (output 156 MB SoA)
| Variant | Throughput |
|---|---|
| scalar | 22.5 GB/s |
| NEON 4-wide | 16.5 GB/s |
| NEON parallel 10 cores | 47.8 GB/s |
| tiled NEON single | 30.8 GB/s |
| tiled NEON parallel 10 cores | **108.0 GB/s** |
| GPU (full pipeline + readback) | 61 ms |

### Shadow Mask (output 13 MB)
| Variant | Throughput / Time |
|---|---|
| scalar cardinal | 8.1 GB/s |
| NEON cardinal | 8.1 GB/s |
| NEON parallel cardinal | **55.4 GB/s** |
| scalar diagonal | 3.1–3.4 GB/s |
| NEON parallel diagonal | 26.3 GB/s |
| GPU | 26 ms (CPU wins ~17×) |

### CPU Renderer (2000×900)
| Variant | Time |
|---|---|
| NEON single-thread | 790 ms |
| scalar parallel | 84 ms |
| NEON parallel | 94 ms |

### Single-Image Render (8000×2667)
| Variant | Time | Speedup |
|---|---|---|
| CPU parallel | 1.26 s | 1× |
| GPU buffer | 130 ms | 9.7× |
| GPU texture | 170 ms | 7.4× |
| GPU combined | **90 ms** | **14.0×** |

### Multi-Frame Steady-State (8000×2667)
| Variant | ms/frame | Speedup |
|---|---|---|
| CPU parallel | 1730.8 | 1× |
| GPU separate | 133.4 | 13.0× |
| GPU combined | 120.4 | 14.4× |
| GPU scene | **98.0** | **17.7×** |

---

## 13. Common Errors

| Error | Cause | Fix |
|---|---|---|
| All black | `hm.data` is `Vec<i16>`, uploaded as raw bytes | Convert: `hm.data.iter().map(\|&v\| v as f32)` |
| Banded stripes | `RgbImage` (3 bytes) with RGBA buffer (4 bytes) | Use `RgbaImage::from_raw` |
| Wrong terrain shape | `pos.y / dx_meters` instead of `dy_meters` | Separate `dx_meters` / `dy_meters` uniforms |
| Uniform size mismatch | `vec3` needs 4-byte pad; struct not multiple of 16 | Add `_pad` fields; verify `size_of` |
| Arc artifacts in foreground | No binary search refinement | Add `binary_search_hit` between step N-1 and N |
| GPU resources freed too early | Buffer dropped while bind group still references address | Store all bound resources in same struct; `_` prefix for lifetime-only fields |
| Entry point not found | Empty shader file | Verify `@compute fn main(...)` is present |
| Camera wrong after resize | `cam` not rebuilt after changing `pic_width` | `let mut cam`; rebuild with new aspect ratio |
