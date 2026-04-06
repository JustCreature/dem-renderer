# Phase 5 — GPU Renderer: Comprehensive Student Textbook

## Part 1: What Is a GPU and Why Does It Exist?

### 1.1 The Motivation

A CPU is designed for **latency**: execute one thread of code as fast as possible. It spends most of its transistor budget on branch predictors, out-of-order execution engines, large caches, and speculative execution — all mechanisms to hide the latency of any single instruction. A modern CPU core can retire ~4 instructions per cycle and has a ROB of 400–600 entries to keep the pipeline fed.

A GPU is designed for **throughput**: execute as many independent threads as possible simultaneously, accepting that each individual thread runs slower. The insight behind the GPU design is that many workloads — graphics, image processing, physics simulation, machine learning — consist of millions of **identical, independent computations** on different data. For those workloads, 10,000 slow parallel threads beats 1 fast sequential thread by a factor of 100×.

Terrain raymarching is exactly this workload. Each output pixel requires an independent ray to be marched through the heightmap. There are no dependencies between pixels. A GPU dispatches all 21 million pixels of an 8000×2667 image simultaneously.

### 1.2 The GPU Execution Model

The fundamental unit of GPU execution is the **SIMD group** (also called warp on NVIDIA, wavefront on AMD). On Apple Silicon, a SIMD group is 32 threads. On NVIDIA it is also 32. On AMD it is 64.

All 32 threads in a SIMD group execute **the same instruction at the same time**, on different data. This is SIMT: Single Instruction, Multiple Threads. It looks like independent threads from the programmer's perspective, but the hardware executes them in lockstep.

A **workgroup** (also called thread block on CUDA) is a larger grouping of threads that share a scratchpad memory called **shared memory** or **threadgroup memory**. In this phase we used 8×8 = 64 threads per workgroup = 2 SIMD groups per workgroup on Apple Silicon.

The GPU scheduler fills execution units by running many SIMD groups simultaneously. When one SIMD group stalls on a memory access (400–800 cycle latency to VRAM), the scheduler instantly switches to another ready SIMD group. This **latency hiding** is how GPUs tolerate high memory latency: not by making memory faster, but by having enough concurrent work to fill the stall time.

### 1.3 Divergence

The key weakness of the SIMT model is **divergence**: when threads in the same SIMD group take different branches (different `if` paths, different loop iteration counts), the hardware must execute both paths sequentially with the inactive lanes masked to zero. A 32-thread SIMD group that splits 50/50 on an `if`/`else` executes at half efficiency.

In our raymarcher, rays in the same 8×8 workgroup may hit terrain at different step counts. Threads that hit early become inactive while others continue marching. This is the GPU analog of SIMD lane divergence on the CPU.

---

## Part 2: wgpu and the GPU Pipeline

### 2.1 What Is wgpu?

wgpu is a Rust implementation of the WebGPU API — a modern, portable GPU API designed to run on top of Metal (Apple), Vulkan (Linux/Android), and D3D12 (Windows). It provides safe Rust bindings to GPU operations without requiring `unsafe` in the user code (the `unsafe` is inside wgpu itself).

The WebGPU API is deliberately minimal and explicit — you allocate every buffer, describe every binding, compile every shader, and assemble every command yourself. This verbosity is intentional: it maps closely to how the GPU hardware actually works.

### 2.2 The wgpu Initialization Chain

Every GPU operation requires working through this chain:

```
Instance → Adapter → Device + Queue
```

- **Instance**: the wgpu entry point. Detects which backend (Metal/Vulkan/D3D12) to use.
- **Adapter**: a handle to a specific physical GPU. `request_adapter` queries all available GPUs and returns one (you can specify `HighPerformance` to prefer the discrete GPU over integrated).
- **Device**: a logical connection to the adapter. This is the object you use to allocate buffers, create shaders, and build pipelines. `request_device` is async because the GPU driver may need to initialize internal state.
- **Queue**: the submission queue. You record commands into a `CommandEncoder`, then `queue.submit()` sends them to the GPU.

In our code:
```rust
let instance = wgpu::Instance::default();
let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions {
    power_preference: wgpu::PowerPreference::HighPerformance,
    ..Default::default()
}).await.expect("no GPU adapter found");
let (device, queue) = adapter
    .request_device(&wgpu::DeviceDescriptor::default())
    .await
    .expect("failed to get device");
```

### 2.3 Async in a Sync Context

wgpu's initialization is async (it involves driver calls that may block). Our `render_gpu` function is synchronous. The bridge is `pollster::block_on(async { ... })` — a minimal async executor that blocks the calling thread until the async block completes. This is appropriate for a one-shot render function. For a real-time renderer you would use a proper async runtime (tokio) and await in a non-blocking context.

### 2.4 Buffers

A GPU **buffer** is a region of GPU-accessible memory. You cannot directly hand the GPU a Rust `Vec` — you must allocate a buffer with `device.create_buffer` and copy data into it.

Buffers have **usage flags** that tell the GPU driver what operations are legal:

| Usage | Meaning |
|---|---|
| `STORAGE` | Shader can read/write |
| `COPY_SRC` | Can be source of a GPU-to-GPU copy |
| `COPY_DST` | Can be destination of a GPU-to-GPU copy |
| `UNIFORM` | Shader can read as a uniform (small, fast, read-only) |
| `MAP_READ` | CPU can map (read) this buffer after GPU writes |

A key restriction: you cannot map a `STORAGE` buffer for CPU reading. You must copy it to a `MAP_READ` buffer first. This is why we have two output buffers:

```rust
// GPU writes here during compute
output_buffer: STORAGE | COPY_SRC

// GPU copies here after compute; CPU reads this
readback_buffer: MAP_READ | COPY_DST
```

The copy happens in the command encoder:
```rust
encoder.copy_buffer_to_buffer(&output_buffer, 0, &readback_buffer, 0, size);
```

### 2.5 `create_buffer_init` vs `create_buffer`

`create_buffer_init` (from the `DeviceExt` trait) creates a buffer **and** uploads initial data in one call. `create_buffer` creates an empty buffer. Use `create_buffer_init` for data you already have (heightmap, normals, camera), and `create_buffer` for outputs where there is no initial data.

---

## Part 3: Uniform Buffers and std140 Layout

### 3.1 What Is a Uniform Buffer?

A **uniform buffer** is a small, read-only, GPU-accessible buffer optimized for data that is the same for all threads in a dispatch — camera parameters, sun direction, image dimensions. Unlike storage buffers, uniforms are cached aggressively and have low-latency access.

The GPU reads our `CameraUniforms` struct from a uniform buffer. The CPU writes it. For this to work, the byte layout of the struct in CPU memory must exactly match what the GPU shader expects.

### 3.2 std140 Alignment Rules

WGSL (and OpenGL std140, and HLSL's cbuffer layout) imposes strict alignment rules on uniform buffer fields:

- `f32` or `u32` or `i32`: 4 bytes, aligned to 4 bytes
- `vec2<f32>`: 8 bytes, aligned to 8 bytes
- **`vec3<f32>`: 12 bytes of data, but aligned to 16 bytes** ← the critical rule
- `vec4<f32>`: 16 bytes, aligned to 16 bytes
- A struct: its total size must be a multiple of 16 bytes

This means a `vec3<f32>` in WGSL always occupies 16 bytes (12 bytes of data + 4 bytes of implicit padding). If your Rust struct places a `[f32; 3]` without padding, the WGSL struct expects 4 more bytes before the next field.

Our `CameraUniforms` struct:
```rust
#[repr(C)]
pub struct CameraUniforms {
    pub origin: [f32; 3],   // 12 bytes
    pub _pad0: f32,         // 4 bytes → 16 bytes total for this vec3
    pub forward: [f32; 3],  // 12 bytes
    pub _pad1: f32,         // 4 bytes padding
    // ... etc
    pub step_m: f32,
    pub t_max: f32,
    pub _pad5: f32,         // \
    pub _pad6: f32,         //  \
    pub _pad7: f32,         //   } pad struct to multiple of 16 bytes
    pub _pad8: f32,         //  /
    pub _pad9: f32,         // /
    pub _pad10: f32,        //
}
```

Total size: 144 bytes = 9 × 16 ✓

### 3.3 `#[repr(C)]`, `bytemuck::Pod`, `bytemuck::Zeroable`

- **`#[repr(C)]`**: tells the Rust compiler to lay out the struct fields in declaration order, with C-compatible alignment rules. Without this, Rust may reorder fields for optimal packing, breaking the GPU layout match.
- **`bytemuck::Pod`** ("Plain Old Data"): a trait that certifies the type contains no padding bytes, no pointers, no references — it can be safely reinterpreted as raw bytes. `bytemuck::bytes_of(&cam)` gives `&[u8]` without any unsafe code.
- **`bytemuck::Zeroable`**: certifies that all-zero bytes is a valid value for the type (required by `Pod`).

These derives let you write:
```rust
contents: bytemuck::bytes_of(&cam),
```
instead of unsafe transmutation.

### 3.4 Debugging Uniform Buffer Mismatches

When the uniform buffer size doesn't match what the shader expects, wgpu emits a validation error like:
```
Uniform buffer binding size 116 is less than minimum 128
```

The number tells you exactly how many bytes the shader expects. Count your Rust struct fields × their sizes, add padding to reach the target, and verify the WGSL struct matches byte-for-byte.

---

## Part 4: Bind Groups and the Binding Model

### 4.1 What Is a Bind Group?

A bind group is the mechanism that connects CPU-side resources (buffers, textures, samplers) to shader-side variable names. Every resource the shader reads or writes must be registered in a bind group before the dispatch.

The binding model has two parts that must match:

1. **BindGroupLayout**: describes the *types* of bindings (buffer, texture, sampler), their binding numbers, and their visibility (which shader stages can see them). This is like a schema.

2. **BindGroup**: assigns actual resources to the slots described by the layout. This is like filling in the schema with real values.

```
Shader: @group(0) @binding(1) var<storage, read> hm: array<f32>;
         ↑ group  ↑ binding   ↑ type

Layout entry: binding=1, ty=Storage{read_only: true}
BindGroup entry: binding=1, resource=hm_buffer.as_entire_binding()
```

All three must agree. Mismatches cause runtime validation errors.

### 4.2 Our Binding Table

For the buffer version:

| Binding | Resource | Type | Shader name |
|---------|----------|------|-------------|
| 0 | cam_buffer | Uniform | `cam` |
| 1 | hm_buffer | Storage (read) | `hm` |
| 2 | output_buffer | Storage (read-write) | `output` |
| 3 | nx_buffer | Storage (read) | `nx` |
| 4 | ny_buffer | Storage (read) | `ny` |
| 5 | nz_buffer | Storage (read) | `nz` |
| 6 | shadow_buffer | Storage (read) | `shadow` |

For the texture version (Phase 5 experiment):

| Binding | Resource | Type | Shader name |
|---------|----------|------|-------------|
| 0 | cam_buffer | Uniform | `cam` |
| 1 | hm_texture view | Texture (Float, D2) | `hm_tex` |
| 2 | hm_sampler | Sampler (NonFiltering) | `hm_sampler` |
| 3 | output_buffer | Storage (read-write) | `output` |
| 4–7 | nx/ny/nz/shadow | Storage (read) | same |

---

## Part 5: The WGSL Shader

### 5.1 Shader Basics

A **shader** is a program that runs on the GPU. In wgpu, shaders are written in WGSL (WebGPU Shading Language). A compute shader is decorated with `@compute` and specifies a workgroup size:

```wgsl
@compute
@workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) { ... }
```

`global_invocation_id` gives each thread its unique (x, y, z) position across the entire dispatch. For a 2D image dispatch, `gid.x` = pixel column and `gid.y` = pixel row.

The dispatch call:
```rust
pass.dispatch_workgroups((width + 7) / 8, (height + 7) / 8, 1);
```
launches enough 8×8 workgroups to cover the entire image. The `+7)/8` is ceiling division — ensures coverage even when dimensions aren't multiples of 8. Threads outside the image bounds are discarded immediately by the bounds check at the top of `main`.

### 5.2 Ray Direction Computation

The ray direction follows the same math as `render_cpu::Camera::ray_for_pixel`:

```wgsl
let u = (f32(gid.x) + 0.5) / f32(cam.img_width);   // [0, 1]
let v = (f32(gid.y) + 0.5) / f32(cam.img_height);   // [0, 1]
let ndc_x = -(2.0 * u - 1.0);  // flip horizontal: left = +, right = -
let ndc_y = 1.0 - 2.0 * v;     // flip vertical: top = +, bottom = -
let dir = normalize(cam.forward + cam.right * ndc_x * cam.half_w + cam.up * ndc_y * cam.half_h);
```

`half_w = tan(hfov/2)` and `half_h = half_w / aspect` are computed on the CPU and stored in the uniform. This scales the right/up vectors to span the correct field of view.

### 5.3 The March Loop

```wgsl
loop {
    let col = i32(pos.x / cam.dx_meters);
    let row = i32(pos.y / cam.dy_meters);
    if col < 0 || row < 0 || col >= i32(cam.hm_cols) || row >= i32(cam.hm_rows) { break; }
    if t > cam.t_max { break; }
    let h = hm[u32(row) * cam.hm_cols + u32(col)];
    if pos.z <= h {
        hit = true;
        pos = binary_search_hit(t - cam.step_m, t, dir, 8);
        break;
    }
    t += cam.step_m;
    pos = cam.origin + dir * t;
}
```

Every thread runs this loop independently. Threads in the same workgroup will diverge when they hit at different step counts. The GPU hardware masks out the done threads but continues executing until all threads in the SIMD group finish.

### 5.4 Binary Search Refinement

Without refinement, all rays that hit at step N are placed at exactly the same distance from the camera — creating concentric arc artifacts visible in the foreground where step size is large relative to terrain detail.

The fix: once a ray crosses below the terrain (step N), binary-search between step N-1 (above) and step N (below) to find the precise surface intersection:

```wgsl
fn binary_search_hit(t_lo_in: f32, t_hi_in: f32, dir: vec3<f32>, iterations: i32) -> vec3<f32> {
    var t_lo = t_lo_in;
    var t_hi = t_hi_in;
    for (var i = 0; i < iterations; i++) {
        let t_mid = (t_lo + t_hi) * 0.5;
        let p_mid = cam.origin + dir * t_mid;
        let c = i32(p_mid.x / cam.dx_meters);
        let r = i32(p_mid.y / cam.dy_meters);
        let h_mid = textureLoad(hm_tex, vec2<i32>(c, r), 0).r;  // texture version
        if p_mid.z <= h_mid { t_hi = t_mid; } else { t_lo = t_mid; }
    }
    return cam.origin + dir * t_lo;
}
```

8 iterations gives sub-meter precision (step_m / 2^8 ≈ 0.1m).

Note that `binary_search_hit` accesses the global `cam` and `hm`/`hm_tex` bindings directly — WGSL functions share the global binding namespace with the entry point that calls them.

### 5.5 Shading

```wgsl
let normal = vec3<f32>(nx[idx], ny[idx], nz[idx]);
let normalized_sun_dir = normalize(cam.sun_dir);
let diffuse = max(0.0, dot(normal, normalized_sun_dir));
let ambient = 0.15;
let in_shadow: f32 = shadow[idx];
let shadow_factor: f32 = 0.5 + 0.5 * in_shadow;
let brightness = (ambient + (1.0 - ambient) * diffuse) * shadow_factor;
```

Lambertian diffuse shading: `dot(normal, sun_dir)` gives the cosine of the angle between the surface normal and the sun direction. `max(0, ...)` clamps negative values (backlit faces). The shadow factor scales between 0.5 (in shadow) and 1.0 (fully lit).

---

## Part 6: Textures vs Storage Buffers

### 6.1 What Is a Texel?

A **texel** (texture element) is the GPU equivalent of a pixel in a texture. Where a pixel is an output display unit, a texel is one addressable cell in a GPU texture. For our 3601×3601 heightmap texture with `R32Float` format, each texel holds one `f32` elevation value.

### 6.2 Texture Format: `R32Float`

The format name encodes the channel layout and type:
- `R` = one channel (red channel only; we don't need RGB for a scalar elevation)
- `32` = 32 bits per channel
- `Float` = IEEE 754 float

Other formats: `Rgba8Unorm` (4 channels, 8-bit, 0.0–1.0 normalized, typical for color images), `Depth32Float` (depth buffer), `Rgba16Float` (HDR).

### 6.3 Texture Parameters

- **`mip_level_count: 1`**: mipmaps are pre-downsampled copies at half, quarter, etc. resolution. We don't want mipmaps — we want exact elevation values, not averaged-down versions. Always 1 for data textures.
- **`sample_count: 1`**: MSAA sample count. 1 = no multisampling. For a compute shader reading data, always 1.
- **`dimension: D2`**: 2D texture (row × column). Also available: D1 (line) and D3 (volume).

### 6.4 Uploading to a Texture: `queue.write_texture`

CPU and GPU have separate memory spaces (even on unified-memory Apple Silicon, wgpu manages ownership explicitly). You cannot hand the GPU a Rust `Vec` directly. `queue.write_texture` schedules a DMA transfer:

```rust
queue.write_texture(
    hm_texture.as_image_copy(),          // destination: which texture, mip, origin
    bytemuck::cast_slice(&hm_f32),       // data: raw bytes
    wgpu::TexelCopyBufferLayout {        // layout: how data is arranged in the byte slice
        offset: 0,
        bytes_per_row: Some(hm.cols as u32 * 4),  // one f32 per texel = 4 bytes per column
        rows_per_image: None,
    },
    wgpu::Extent3d { width: hm.cols as u32, height: hm.rows as u32, depth_or_array_layers: 1 },
);
```

`bytes_per_row` must be provided for 2D textures — it tells wgpu the stride between rows in the source data. Since our `hm_f32` is a flat row-major array with no padding, `bytes_per_row = cols * 4`.

### 6.5 Accessing Textures in WGSL: `textureLoad`

```wgsl
@group(0) @binding(1) var hm_tex: texture_2d<f32>;
@group(0) @binding(2) var hm_sampler: sampler;

let h = textureLoad(hm_tex, vec2<i32>(col, row), 0).r;
```

`textureLoad` reads a specific texel by integer coordinates (no interpolation). Arguments:
- texture handle
- `vec2<i32>(col, row)`: x = column, y = row
- `0`: mip level
Returns `vec4<f32>` — the `.r` extracts the first (and only) channel.

Contrast with `textureSampleLevel` which takes normalized UV coordinates (0.0–1.0) and can apply bilinear interpolation. For a heightmap where you want exact values, `textureLoad` is correct.

### 6.6 The Texture Cache: Why It's Different from a Storage Buffer

The GPU **texture cache** is a dedicated L1 cache (typically 4–16 KB per shader array on Apple Silicon) designed specifically for 2D spatial access patterns. Its key property: it stores data in an internal 2D-tiled (Morton/Z-order) layout, so a 4×4 neighborhood of texels fits within a single cache line. When multiple threads in the same workgroup access neighboring texels, they all hit the same texture cache line.

A **storage buffer** has a linear cache. The 1D index `row * cols + col` maps 2D coordinates to a 1D address. Vertically adjacent accesses (`row` changes by 1) are `cols * 4 ≈ 14 KB` apart — completely different cache lines, even if they're only one texel apart spatially.

---

## Part 7: Texture vs Buffer — Experimental Results

### 7.1 The Hypothesis

With the texture cache's 2D-aware layout, reads by nearby threads should produce more cache hits than storage buffer reads. Rays in the same 8×8 workgroup sample geographically nearby terrain → spatial locality → texture cache hits.

### 7.2 Results

All measurements at 8000×2667 pixels, M4 Max:

| step_m divisor | step_m | Buffer | Texture | Texture saving (abs) | Advantage (%) |
|---|---|---|---|---|---|
| /0.8 | 26m | 0.13s | 0.11s | 0.02s | 18% |
| /2.0 | 15m | 0.23s | 0.19s | 0.04s | 17% |
| /4.0 | 7.5m | 0.36s | 0.31s | 0.05s | 14% |
| /8.0 | 3.7m | 0.64s | 0.59s | 0.05s | 8% |

### 7.3 Analysis: What the Data Shows

**The absolute saving plateaus at ~0.05s** as step count grows. The percentage advantage *shrinks* from 18% to 8%. This disproves the naive hypothesis that finer steps → more cache reuse → larger texture advantage.

**What is actually happening:**

The texture cache benefit comes from **cross-thread spatial locality** at the start of ray traversal, not from intra-ray temporal reuse. When 64 threads in an 8×8 workgroup begin marching, they all start from nearby pixel positions and sample nearby terrain. The texture cache captures this 2D neighborhood in one or a few cache lines. The storage buffer's linear cache does not.

As rays travel deeper into the scene, they **diverge** — different rays hit at different terrain features, sampling different heightmap regions. At this stage, neither cache helps effectively. The ~0.05s fixed saving represents exactly the early-traversal phase where workgroup threads are still geographically close.

**Both variants scale linearly with step count**, confirming the bottleneck is proportional to the number of memory accesses (compute-bound, not cache-bound). Neither has hit a bandwidth ceiling.

### 7.4 The Lesson

The texture cache advantage is real but fixed in absolute terms (~0.05s for this scene). It does not compound with more work. For workloads with dense 2D reuse (e.g., image convolution, upsampling), the texture cache advantage would be much larger. For raymarching, where rays quickly diverge after the initial few steps, it provides a moderate one-time benefit.

---

## Part 8: GPU vs CPU Performance

### 8.1 Numbers at Scale

At 8000×2667 = 21.3 million pixels, step_m = dx/0.8:

| Renderer | Time | Pixels/sec |
|---|---|---|
| CPU scalar (1 thread) | ~8.5s | ~2.5 M/s |
| CPU parallel (10 cores) | 1.03s | ~20.7 M/s |
| GPU buffer | 0.13s | ~164 M/s |
| GPU texture | 0.11s | ~194 M/s |

GPU texture is **8.4× faster than 10-core parallel CPU** and **~75× faster than scalar CPU**.

### 8.2 Why the GPU Wins at Scale

Apple M4 Max GPU has 40 GPU cores (shader arrays). At 8×8 workgroups = 64 threads/workgroup = 2 SIMD groups/workgroup, the GPU can run thousands of rays simultaneously. The CPU runs 10.

The workload is embarrassingly parallel — every pixel independent, no synchronization. This is ideal for GPU.

### 8.3 The Crossover Point

At 2000×900 = 1.8M pixels: GPU ≈ CPU parallel at ~0.08s each. The GPU has a fixed initialization overhead (~50–80ms from `Instance::default()` through pipeline compilation). Below ~5M pixels, this overhead dominates and the GPU advantage disappears.

For a real-time renderer, you would initialize once and reuse the device/pipeline across frames, eliminating this overhead entirely. Our one-shot implementation pays the full init cost every call.

---

## Part 9: Common Errors Encountered

### 9.1 All-Black Output
**Cause**: heightmap data was `Vec<i16>`, not `Vec<f32>`. Uploading `i16` bytes reinterpreted as `f32` gives garbage elevation values.
**Fix**: `let hm_f32: Vec<f32> = hm.data.iter().map(|&v| v as f32).collect();`

### 9.2 Banded/Striped Output
**Cause**: `image::RgbImage::from_raw` (3 bytes/pixel) used with a 4-bytes/pixel RGBA buffer.
**Fix**: use `image::RgbaImage::from_raw`.

### 9.3 Terrain Completely Wrong Shape
**Cause**: `pos.y` was divided by `dx_meters` instead of `dy_meters` in the shader. Row index was wrong by a constant factor, placing rays outside the heightmap bounds immediately.
**Fix**: use separate `dx_meters` and `dy_meters` uniforms.

### 9.4 Uniform Buffer Size Mismatch
**Cause**: adding fields to `CameraUniforms` without adding corresponding padding to reach a multiple of 16 bytes.
**Fix**: count bytes carefully; add `_pad` fields until `std::mem::size_of::<CameraUniforms>()` is a multiple of 16.

### 9.5 Arc Artifacts
**Cause**: all rays hitting at step N are placed at exactly distance `N * step_m`, creating concentric iso-step arcs visible in the foreground.
**Fix**: binary search refinement between step N-1 and step N.

### 9.6 Panoramic Image Mismatch
**Cause**: `cam` variable was built with 2000px width, then `pic_width` was changed to 8000, but `cam` was not rebuilt. The GPU got 8000/900 aspect but the CPU render used 2000/900 aspect.
**Fix**: declare `cam` as `let mut`, rebuild it after changing `pic_width`.

### 9.7 Entry Point 'main' Not Found
**Cause**: `shader.wgsl` was empty (accidentally created as an empty file).
**Fix**: verify shader file content; add `@compute @workgroup_size(8,8,1) fn main(...)`.

---

## Summary

Phase 5 built a complete GPU compute renderer using wgpu/WGSL in Rust. The GPU execution model (SIMT, SIMD groups, divergence, latency hiding) was explored through the raymarching workload. The texture vs storage buffer experiment revealed that the texture cache provides a real but fixed benefit (~18%) from cross-thread 2D spatial locality, not from intra-ray temporal reuse. At 21M pixels, the GPU is 8.4× faster than 10-core parallel CPU — the crossover from wgpu init overhead occurs around 3–5M pixels.
