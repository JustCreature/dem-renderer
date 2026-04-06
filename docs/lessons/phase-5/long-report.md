# Phase 5 — GPU Renderer: Comprehensive Student Textbook

**Hardware**: Apple M4 Max (10 performance cores, 40 GPU shader arrays, 400 GB/s unified memory)
**Dataset**: USGS SRTM 1-arc-second, Hintertux N47E011, 3601×3601 cells (~52 MB as f32)
**Build**: `cargo build --release`, `opt-level=3`, `lto="thin"`, `codegen-units=1`

---

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
- **Device**: a logical connection to the adapter. This is the object you use to allocate buffers, create shaders, and build pipelines.
- **Queue**: the submission queue. You record commands into a `CommandEncoder`, then `queue.submit()` sends them to the GPU.

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

**Fixed initialization overhead: ~80ms.** This cost dominates at small image sizes. At 2000×900 = 1.8M pixels, the GPU render time is ~80ms — entirely initialization, essentially zero compute. Above ~5M pixels, GPU compute begins to dominate.

### 2.3 The GpuContext Pattern: Amortizing Init Cost

The first-generation design called `Instance → Adapter → Device` on every render call. For animations (60 frames), this paid 80ms × 60 = 4.8 seconds in initialization alone.

The fix is a **`GpuContext` struct** that owns the `Device` and `Queue` and is created once:

```rust
pub struct GpuContext {
    pub device: wgpu::Device,
    pub queue:  wgpu::Queue,
}
```

All render functions take `&GpuContext` instead of creating their own device. The `~80ms` init cost is paid once, then amortized across every subsequent render call.

This is the same pattern as a database connection pool: the expensive connection is established once and shared across queries. Creating a new connection per query would be catastrophically slow.

### 2.4 Async in a Sync Context

wgpu's initialization is async (it involves driver calls that may block). Our render functions are synchronous. The bridge is `pollster::block_on(async { ... })` — a minimal async executor that blocks the calling thread until the async block completes. This is appropriate for a one-shot render function. For a real-time renderer you would use a proper async runtime (tokio) and await in a non-blocking context.

### 2.5 Buffers

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

### 2.6 `create_buffer_init` vs `create_buffer`

`create_buffer_init` (from the `DeviceExt` trait) creates a buffer **and** uploads initial data in one call. `create_buffer` creates an empty buffer. Use `create_buffer_init` for data you already have (heightmap, normals, camera), and `create_buffer` for outputs where there is no initial data.

---

## Part 3: Uniform Buffers and std140 Layout

### 3.1 What Is a Uniform Buffer?

A **uniform buffer** is a small, read-only, GPU-accessible buffer optimized for data that is the same for all threads in a dispatch — camera parameters, sun direction, image dimensions. Unlike storage buffers, uniforms are cached aggressively and have low-latency access.

### 3.2 std140 Alignment Rules

WGSL imposes strict alignment rules on uniform buffer fields:

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
    pub origin:  [f32; 3], pub _pad0:  f32,  // 16 bytes
    pub forward: [f32; 3], pub _pad1:  f32,  // 16 bytes
    pub right:   [f32; 3], pub _pad2:  f32,  // 16 bytes
    pub up:      [f32; 3], pub _pad3:  f32,  // 16 bytes
    pub sun_dir: [f32; 3], pub _pad4:  f32,  // 16 bytes
    pub half_w:  f32, pub half_h: f32, pub img_width: u32, pub img_height: u32, // 16
    pub hm_cols: u32, pub hm_rows: u32, pub dx_meters: f32, pub dy_meters: f32, // 16
    pub step_m:  f32, pub t_max:  f32, pub _pad5: f32, pub _pad6: f32,          // 16
}
```

Total size: 128 bytes = 8 × 16 ✓. This is the **128 bytes written per frame** in `GpuScene`.

### 3.3 `#[repr(C)]`, `bytemuck::Pod`, `bytemuck::Zeroable`

- **`#[repr(C)]`**: tells the Rust compiler to lay out the struct fields in declaration order, with C-compatible alignment rules.
- **`bytemuck::Pod`** ("Plain Old Data"): certifies the type can be safely reinterpreted as raw bytes. `bytemuck::bytes_of(&cam)` gives `&[u8]` without any unsafe code.
- **`bytemuck::Zeroable`**: certifies that all-zero bytes is a valid value for the type.

### 3.4 Debugging Uniform Buffer Mismatches

When the uniform buffer size doesn't match what the shader expects, wgpu emits a validation error like:
```
Uniform buffer binding size 116 is less than minimum 128
```

The number tells you exactly how many bytes the shader expects. Count your Rust struct fields × their sizes, add padding to reach the target, and verify the WGSL struct matches byte-for-byte.

---

## Part 4: Bind Groups and the Binding Model

### 4.1 What Is a Bind Group?

A bind group connects CPU-side resources (buffers, textures, samplers) to shader-side variable names. Every resource the shader reads or writes must be registered in a bind group before the dispatch.

The binding model has two parts that must match:

1. **BindGroupLayout**: describes the *types* of bindings, their binding numbers, and their visibility. This is like a schema.
2. **BindGroup**: assigns actual resources to the slots described by the layout. This is like filling in the schema with real values.

```
Shader: @group(0) @binding(1) var<storage, read> hm: array<f32>;
Layout entry: binding=1, ty=Storage{read_only: true}
BindGroup entry: binding=1, resource=hm_buffer.as_entire_binding()
```

### 4.2 Critical: wgpu Bind Groups Do Not Hold CPU-Side Arc References

A common misconception: "the bind group keeps my buffer alive." It does not. wgpu bind groups store the GPU-side buffer address, not a CPU-side `Arc` reference. If you let the `wgpu::Buffer` value go out of scope, it is dropped — the GPU memory is freed — even if a bind group still references that address. On the next dispatch, the GPU accesses freed memory.

The fix: **all resources referenced by a bind group must live as long as the bind group**. This is why `GpuScene` holds owned fields for every buffer:

```rust
pub struct GpuScene {
    _hm_texture: wgpu::Texture,   // never accessed directly — kept alive only
    _hm_view:    wgpu::TextureView,
    _nx_buf:     wgpu::Buffer,    // normals buffers: pre-computed once, kept alive
    _ny_buf:     wgpu::Buffer,
    _nz_buf:     wgpu::Buffer,
    shadow_buf:  wgpu::Buffer,    // updated occasionally via write_buffer
    cam_buf:     wgpu::Buffer,    // updated every frame via write_buffer
    render_bg:   wgpu::BindGroup, // references all of the above GPU addresses
    // ...
}
```

The `_` prefix on fields like `_hm_texture` signals "this field exists only for its lifetime, not for direct access." Rust drops fields in declaration order; holding them in the struct guarantees they outlive the bind group.

### 4.3 `write_buffer` Updates Contents In-Place, Bind Group Sees New Data Automatically

When you call `queue.write_buffer(&cam_buf, 0, &new_data)`, you update the **contents** of the existing buffer in place. The buffer's GPU address does not change. The bind group that references that address automatically sees the new data on the next dispatch — no need to rebuild the bind group.

This is the key mechanism that makes `GpuScene` efficient: only 128 bytes of camera data cross the CPU→GPU bus per frame, while all static data (52 MB heightmap, 156 MB normals, 13 MB shadow) stays on the GPU untouched.

---

## Part 5: The GpuScene Pattern

### 5.1 The Problem with Per-Call Rendering

Every function like `render_gpu_texture` or `render_gpu_combined` follows the same lifecycle:

1. Create buffers, upload all data (heightmap + normals + shadow = 221+ MB)
2. Create bind group layout, bind group, pipeline
3. Encode commands, dispatch, submit
4. Poll GPU, map readback buffer, copy out pixels

Steps 1 and 2 are pure overhead for animation: the terrain doesn't change between frames. Only the camera moves.

### 5.2 GpuScene: Static Resources Uploaded Once

`GpuScene::new()` does the expensive work once:

```
new() {
    upload heightmap as texture     ← 52 MB, once
    allocate nx/ny/nz storage buffers ← 156 MB, once
    run normals compute pass on GPU  ← fills nx/ny/nz from heightmap, no CPU transfer
    upload shadow_buf               ← 13 MB, once
    allocate cam_buf (128 bytes)    ← uniform, written per frame
    allocate output_buf + readback_buf ← 85 MB output, once
    create render pipeline          ← compiled by Metal driver, once
    create render bind group        ← references all of the above, once
}
```

`render_frame()` then does only:

```
render_frame() {
    write_buffer(cam_buf, 128 bytes)   ← only data transfer
    encode { dispatch render; copy to readback }
    submit → poll → map → copy pixels out
}
```

### 5.3 Normals Computed on GPU, Never Read Back

In `GpuScene::new()`, the normals pipeline runs inside a scoped block:

```rust
{
    let normals_pipeline = device.create_compute_pipeline(...);
    let normals_bg = device.create_bind_group(...);
    let mut enc = device.create_command_encoder(...);
    {
        let mut pass = enc.begin_compute_pass(...);
        pass.set_pipeline(&normals_pipeline);
        pass.set_bind_group(0, &normals_bg, &[]);
        pass.dispatch_workgroups(ceil(cols/16), ceil(rows/16), 1);
    }
    queue.submit([enc.finish()]);
    device.poll(wgpu::Maintain::Wait);
    // normals_pipeline and normals_bg dropped here
    // _nx_buf, _ny_buf, _nz_buf are OUTSIDE this block — they survive
}
```

After this block, only `_nx_buf`, `_ny_buf`, `_nz_buf` remain. The pipeline and its bind group are dropped. The normals data lives permanently on the GPU, consumed by every render frame. **It never crosses the PCIe/unified-memory bus back to CPU.**

This avoids:
- 156 MB readback (nx + ny + nz)
- 156 MB re-upload on every frame
- Re-running the normals compute on every frame

### 5.4 update_shadow() for Sun Animation

The shadow mask is CPU-computed (NEON parallel sweep) and uploaded to the GPU. When the sun moves, the shadow changes. `update_shadow()` allows refreshing the GPU buffer:

```rust
pub fn update_shadow(&self, shadow_mask: &ShadowMask) {
    self.ctx.queue.write_buffer(&self.shadow_buf, 0, shadow_mask.data.as_slice());
}
```

The caller computes a new `ShadowMask` on the CPU, passes it in. `write_buffer` overwrites the existing GPU buffer in-place. The render bind group sees the new shadow data on the next dispatch without rebuilding.

**Why shadow is CPU-computed but normals are GPU-computed:**

| Property | Normals | Shadow |
|---|---|---|
| Dependency between pixels | None — each pixel reads only its 4 neighbours | Serial — running-max along each row |
| GPU parallelism | 100% — all invocations independent | 100% across rows, 0% within a row |
| Best approach | GPU: dispatch one thread per pixel | CPU: NEON parallel across rows |
| Transfer at scene build | 52 MB heightmap in, nothing out | 13 MB shadow out after CPU compute |

---

## Part 6: The WGSL Shader

### 6.1 Shader Basics

A compute shader is decorated with `@compute` and specifies a workgroup size:

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
launches enough 8×8 workgroups to cover the entire image. The `+7)/8` is ceiling division — ensures coverage even when dimensions aren't multiples of 8.

### 6.2 Ray Direction Computation

```wgsl
let u = (f32(gid.x) + 0.5) / f32(cam.img_width);   // [0, 1]
let v = (f32(gid.y) + 0.5) / f32(cam.img_height);   // [0, 1]
let ndc_x = -(2.0 * u - 1.0);  // flip horizontal: left = +, right = -
let ndc_y = 1.0 - 2.0 * v;     // flip vertical: top = +, bottom = -
let dir = normalize(cam.forward + cam.right * ndc_x * cam.half_w + cam.up * ndc_y * cam.half_h);
```

`half_w = tan(hfov/2)` and `half_h = half_w / aspect` are computed on the CPU and stored in the uniform.

### 6.3 The March Loop

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

Every thread runs this loop independently. Threads in the same workgroup diverge when they hit at different step counts.

### 6.4 Binary Search Refinement

Without refinement, all rays hitting at step N are placed at exactly distance `N * step_m` — producing concentric arc artifacts visible in the foreground.

The fix: once a ray crosses below terrain at step N, binary-search between step N-1 (above) and step N (below) to find the precise intersection:

```wgsl
fn binary_search_hit(t_lo_in: f32, t_hi_in: f32, dir: vec3<f32>, iterations: i32) -> vec3<f32> {
    var t_lo = t_lo_in;
    var t_hi = t_hi_in;
    for (var i = 0; i < iterations; i++) {
        let t_mid = (t_lo + t_hi) * 0.5;
        let p_mid = cam.origin + dir * t_mid;
        let c = i32(p_mid.x / cam.dx_meters);
        let r = i32(p_mid.y / cam.dy_meters);
        let h_mid = hm[u32(r) * cam.hm_cols + u32(c)];
        if p_mid.z <= h_mid { t_hi = t_mid; } else { t_lo = t_mid; }
    }
    return cam.origin + dir * t_lo;
}
```

8 iterations gives sub-meter precision (step_m / 2^8 ≈ 0.1m at step_m ≈ 20m).

### 6.5 Shading

```wgsl
let normal = vec3<f32>(nx[idx], ny[idx], nz[idx]);
let diffuse = max(0.0, dot(normal, normalize(cam.sun_dir)));
let ambient = 0.15;
let in_shadow = shadow[idx];
let shadow_factor = 0.5 + 0.5 * in_shadow;
let brightness = (ambient + (1.0 - ambient) * diffuse) * shadow_factor;
```

Lambertian diffuse: `dot(normal, sun_dir)` = cosine of incidence angle. Shadow factor scales between 0.5 (in shadow) and 1.0 (lit). Color is then derived from elevation.

---

## Part 7: Textures vs Storage Buffers

### 7.1 What Is a Texel?

A **texel** (texture element) is one addressable cell in a GPU texture. For our 3601×3601 heightmap with `R32Float` format, each texel holds one `f32` elevation value.

### 7.2 Texture Format: `R32Float`

The format name encodes the channel layout and type:
- `R` = one channel (red; we don't need RGB for a scalar elevation)
- `32` = 32 bits per channel
- `Float` = IEEE 754 float

### 7.3 Accessing Textures in WGSL: `textureLoad`

```wgsl
@group(0) @binding(1) var hm_tex: texture_2d<f32>;
@group(0) @binding(2) var hm_sampler: sampler;

let h = textureLoad(hm_tex, vec2<i32>(col, row), 0).r;
```

`textureLoad` reads a specific texel by integer coordinates (no interpolation). Returns `vec4<f32>` — the `.r` extracts the first channel.

### 7.4 The Texture Cache

The GPU **texture cache** is a dedicated L1 cache (~4–16 KB per shader array on Apple Silicon) that stores data in an internal 2D-tiled (Morton/Z-order) layout. A 4×4 neighborhood of texels fits within a single cache line. When threads in the same workgroup access neighboring texels, they all hit the same cache line.

A **storage buffer** has a linear cache. The 1D index `row * cols + col` maps 2D coordinates to a 1D address. Vertically adjacent accesses (`row` changes by 1) are `cols * 4 ≈ 14 KB` apart — completely different cache lines even if they are spatially adjacent.

The hypothesis: rays in the same workgroup start from nearby screen positions → sample nearby terrain → 2D locality → texture cache hits → texture faster than buffer.

### 7.5 Measured Results (M4 Max, 8000×2667, step_m = dx/1.0)

| Variant      | Single-shot | Multi-frame steady-state | vs CPU parallel |
|--------------|-------------|--------------------------|-----------------|
| CPU parallel | 1.26 s      | 1730 ms/frame            | 1×              |
| GPU buffer   | **130 ms**  | ~130 ms                  | **9.7×**        |
| GPU texture  | 170 ms      | **133 ms** (via GPU_separate) | **7.4×**  |

**Buffer wins in single-shot (1.3×). Texture and buffer are roughly equal in multi-frame steady-state.** This contradicts the naive hypothesis.

### 7.6 Why the Texture Cache Didn't Win

For raymarching, rays quickly **diverge** spatially. All 64 threads in an 8×8 workgroup start from nearby screen pixels, but their rays travel in slightly different directions. By the 10th step (~200m into the scene), they're sampling heightmap cells in a broad strip rather than a tight 2D patch. The texture cache's 2D locality advantage only applies during the first few steps.

Additionally, `textureLoad` routes through the **sampler unit** — a hardware block with its own pipeline latency (~20–40 cycles on Apple GPU). A storage buffer read goes directly to the L1 data cache without that detour. For our case where bilinear interpolation is not needed (we call `textureLoad` with integer coords), the sampler unit is pure overhead.

The texture path also has a slightly higher per-frame cost in single-shot measurements because the Metal driver caches the texture binding differently — it shows up as 170ms vs 130ms in first-call scenarios but equalises (~133ms) after the pipeline is fully warmed.

---

## Part 8: Multi-Frame Benchmark

### 8.1 Setup

10 frames, 8000×2667, camera panning 200m east per frame. Each variant has a warmup call before timing. This measures **steady-state per-frame cost**, not first-call cost.

### 8.2 Results

| Variant           | ms/frame | vs CPU  | vs GPU separate |
|-------------------|----------|---------|-----------------|
| CPU parallel      | 1730.8   | 1×      | —               |
| GPU separate      | 133.4    | 13.0×   | 1×              |
| GPU combined      | 120.4    | 14.4×   | 1.11×           |
| GPU scene         | **98.0** | **17.7×** | **1.36×**     |

### 8.3 What Each Variant Pays Per Frame

**CPU parallel** (1730 ms): full raymarcher on 10 cores, no GPU. 0.6 fps for animation.

**GPU separate** (133 ms): re-uploads heightmap (52 MB) + normals (156 MB) + shadow (13 MB) = 221 MB every frame. Rebuilds bind groups. Time breakdown:
- ~80 ms: data uploads (221 MB)
- ~40 ms: compute dispatch + readback
- ~13 ms: command encoding overhead

**GPU combined** (120 ms): skips normal map upload. Computes normals on GPU from the heightmap (which is still uploaded every frame). Time breakdown:
- ~30 ms: heightmap + shadow uploads (65 MB)
- ~40 ms: normals compute pass on GPU
- ~40 ms: render pass + 85 MB readback
- ~10 ms: overhead

**GPU scene** (98 ms): uploads nothing except 128 bytes of camera data. Time breakdown:
- ~0 ms: `write_buffer(cam_buf, 128 bytes)`
- ~5 ms: command encoding
- ~88 ms: render dispatch + readback (85 MB RGBA)
- ~5 ms: wgpu map_async + poll overhead

### 8.4 The Readback Floor

The hard floor for GPU scene is the **readback**: 85 MB of RGBA pixels must travel from GPU to CPU to be encoded into a GIF or PNG. At 400 GB/s theoretical bandwidth that is 0.2 ms — but actual buffer mapping involves page faults, driver synchronisation, and cache invalidation. Empirically this is 80–100 ms.

Removing the readback would require a swap-chain / display-pipeline architecture where frames are presented directly to a display without CPU involvement. That's a different rendering model than what we built here.

### 8.5 Speedup Decomposition

| Step | ms saved | Running speedup vs CPU |
|------|----------|------------------------|
| CPU → GPU separate | 1597 ms | 13.0× |
| GPU separate → combined | 13 ms | 14.4× |
| GPU combined → scene | 22 ms | 17.7× |

The 13× jump from CPU to GPU is the parallelism win. Everything after that is reducing overhead. The readback dominates the remaining 98ms budget.

---

## Part 9: GPU vs CPU — Which Wins and Why

### 9.1 Numbers at 8000×2667 = 21.3 Mpix

| Renderer | Time | Mpix/s | Speedup |
|---|---|---|---|
| CPU NEON single-thread | 790 ms (2000×900 only) | — | — |
| CPU parallel 10 cores | 1260 ms | 17.0 | 1× |
| GPU buffer | 130 ms | 164 | **9.7×** |
| GPU combined (normals on GPU) | 90 ms | 237 | **14.0×** |
| GPU scene (persistent state) | 98 ms/frame | 217 | **12.9× per-frame** |

### 9.2 Why GPU Wins at Rendering

Raymarching is **embarrassingly parallel**: every pixel is independent, no synchronization, no data dependencies between pixels. The GPU dispatches all 21.3M pixels simultaneously. CPU runs 10 threads × ~2M pixels = 10 threads.

Apple M4 Max has 40 GPU shader arrays (GPU cores). Each shader array has 8 SIMD groups in flight. 40 × 8 × 32 threads = ~10,000 threads simultaneously. CPU has 10 threads. For the same amount of work, GPU has 1000× more parallelism.

The GPU doesn't win 1000×, it wins 10–17×, because:
- Each GPU thread has lower clock speed (hundreds of MHz vs 4+ GHz on CPU)
- The 85 MB readback costs ~88 ms and is unavoidable
- First-call pipeline compilation adds overhead

### 9.3 Why CPU Wins at Shadow Computation

Shadow sweep has a **serial running-max dependency** along each row: `max_angle[col] = max(max_angle[col-1], ...)`. You cannot compute position N until position N-1 is done. On the GPU, each row would still be processed serially. The GPU shader cores run at ~1/4 the clock rate of a CPU performance core, so the serial portion runs slower on GPU.

| Task | Best CPU | GPU time | Winner |
|------|----------|----------|--------|
| Shadow (cardinal) | 1.5 ms (NEON parallel) | 26 ms | **CPU 17×** |
| Shadow (diagonal) | 3.8 ms (NEON parallel) | n/a | **CPU** |
| Render 8000×2667 | 1260 ms | 90–130 ms | **GPU 10–14×** |

CPU wins shadow by the same factor GPU wins rendering. The decisive property is whether the algorithm has serial dependencies. A future parallel prefix scan on the GPU could match or beat the CPU for cardinal shadows — but the serial diagonal sweep remains CPU territory.

### 9.4 Normals: Both Fast, Different Trade-offs

| Variant | Throughput / Time | Notes |
|---------|-------------------|-------|
| CPU scalar | 22.5 GB/s | Cold cache; auto-vectorised |
| CPU NEON parallel | 47.8 GB/s | 10 cores |
| CPU tiled NEON parallel | **108 GB/s** | L2-served reads |
| GPU compute | 61 ms total | ~50 ms readback; actual compute is sub-ms |

The GPU normals result (61 ms) includes 156 MB of readback — the actual shader dispatch is <1 ms. In `GpuScene` the normals are computed once and never read back, making the GPU approach strictly better for the animation use case.

---

## Part 10: Complete Benchmark Snapshot (M4 Max, 2026-04-06)

### Memory Bandwidth Baseline (256 MB)

| Pattern | Throughput |
|---|---|
| seq_read SIMD | 65.8 GB/s |
| seq_read scalar | 5.0 GB/s |
| seq_write | 6.1 GB/s |
| random_read SIMD | 1.4 GB/s |
| random_read scalar | 0.6 GB/s |
| random_write | 0.5 GB/s |

### Normal Map (3601×3601, output 156 MB SoA)

| Variant | Throughput |
|---|---|
| scalar | 22.5 GB/s |
| NEON 4-wide | 16.5 GB/s |
| NEON parallel 10 cores | 47.8 GB/s |
| tiled NEON single | 30.8 GB/s |
| tiled NEON parallel 10 cores | **108.0 GB/s** |
| GPU (full pipeline) | 61 ms |

### Shadow Mask (3601×3601, output 13 MB)

| Variant | Throughput | Notes |
|---|---|---|
| scalar cardinal | 8.1 GB/s | |
| NEON cardinal | 8.1 GB/s | Serial dependency cancels SIMD |
| NEON parallel cardinal | **55.4 GB/s** | 10 cores × independent rows |
| scalar diagonal (any azimuth) | 3.1–3.4 GB/s | 2.4× slower than cardinal |
| NEON parallel diagonal | 26.3 GB/s | 2.1× slower than cardinal |
| GPU | 26 ms | Serial shader; CPU wins by ~17× |

### Single-Image Render (8000×2667 = 21.3 Mpix)

| Variant | Time | Speedup |
|---|---|---|
| CPU scalar parallel | 1.26 s | 1× |
| GPU buffer | 130 ms | 9.7× |
| GPU texture | 170 ms | 7.4× |
| GPU combined | **90 ms** | **14.0×** |

### Multi-Frame Render (8000×2667 × 10 frames, steady-state)

| Variant | ms/frame | Speedup vs CPU |
|---|---|---|
| CPU parallel | 1730.8 | 1× |
| GPU separate | 133.4 | 13.0× |
| GPU combined | 120.4 | 14.4× |
| GPU scene | **98.0** | **17.7×** |

---

## Part 11: Common Errors Encountered

### 11.1 All-Black Output
**Cause**: heightmap data was `Vec<i16>`, not `Vec<f32>`. Uploading `i16` bytes reinterpreted as `f32` gives garbage elevation values.
**Fix**: `let hm_f32: Vec<f32> = hm.data.iter().map(|&v| v as f32).collect();`

### 11.2 Banded/Striped Output
**Cause**: `image::RgbImage::from_raw` (3 bytes/pixel) used with a 4-bytes/pixel RGBA buffer.
**Fix**: use `image::RgbaImage::from_raw`.

### 11.3 Terrain Completely Wrong Shape
**Cause**: `pos.y` divided by `dx_meters` instead of `dy_meters` in the shader. Row index wrong by a constant factor.
**Fix**: use separate `dx_meters` and `dy_meters` uniforms.

### 11.4 Uniform Buffer Size Mismatch
**Cause**: adding fields to `CameraUniforms` without adding padding to reach a multiple of 16 bytes.
**Fix**: count bytes; add `_pad` fields until `std::mem::size_of::<CameraUniforms>()` is a multiple of 16.

### 11.5 Arc Artifacts
**Cause**: all rays hitting at step N are placed at exactly `N * step_m`, creating concentric iso-step arcs visible in the foreground.
**Fix**: binary search refinement between step N-1 and step N.

### 11.6 Panoramic Camera Mismatch
**Cause**: `cam` built with 2000px width, then `pic_width` changed to 8000, but `cam` not rebuilt.
**Fix**: declare `cam` as `let mut`; rebuild after changing dimensions.

### 11.7 Entry Point 'main' Not Found
**Cause**: `shader.wgsl` was empty (accidentally created as empty file).
**Fix**: verify shader file content; ensure `@compute @workgroup_size(8,8,1) fn main(...)` is present.

### 11.8 GPU Resources Freed While Bind Group Still References Them
**Cause**: buffer created in a local scope, bind group holds reference to its GPU address, buffer dropped before bind group.
**Fix**: store all resources referenced by a bind group in the same struct that owns the bind group. Use `_` prefix fields for "kept alive only" semantics.

---

## Part 12: Workgroup Size Experiment

### 12.1 The Question

The initial implementation used `@workgroup_size(8, 8, 1)` — 64 threads per workgroup, 2 SIMD groups on Apple Silicon. The obvious questions are:

1. Does **workgroup shape** matter (8×8 vs 16×4 vs 4×16)?
2. Does **total thread count per workgroup** matter (64 vs 256)?

The theory: going from 64 (2 SIMD groups) to 256 (8 SIMD groups) gives the GPU scheduler more concurrent groups to hide memory latency with. This should help a memory-latency-bound workload.

### 12.2 Method

Added `render_gpu_buffer_wg(wx, wy)` — same as `render_gpu_buffer` but patches `@workgroup_size(8, 8, 1)` to `@workgroup_size(wx, wy, 1)` via string replacement at shader compile time, and adjusts the dispatch to `(width + wx - 1) / wx, (height + wy - 1) / wy`. Each variant gets one warm-up call (pipeline compile), then one timed render.

### 12.3 Results (M4 Max, 8000×2667, 2026-04-06)

| Workgroup | Threads | SIMD groups | Time |
|---|---|---|---|
| 8×8 | 64 | 2 | 136.1 ms |
| 16×4 | 64 | 2 | 138.7 ms |
| 4×16 | 64 | 2 | 136.1 ms |
| 32×2 | 64 | 2 | 140.2 ms |
| 16×16 | 256 | 8 | 137.5 ms |
| 32×8 | 256 | 8 | 138.1 ms |
| 8×32 | 256 | 8 | 135.6 ms |

**All variants land within ±3% of each other (~136–140 ms). Workgroup size has no meaningful effect.**

### 12.4 Why Workgroup Size Doesn't Matter Here

**Shape (8×8 vs 16×4 vs 4×16)** doesn't matter because rays diverge after the first few steps. Adjacent screen pixels start near each other in world space but quickly land on different terrain at different distances. There is no sustained 2D spatial locality that a wider or taller workgroup shape could exploit.

**Thread count (64 vs 256)** doesn't matter because the bottleneck is not compute or memory latency within the dispatch — it is the **85 MB RGBA readback** (~88 ms), which happens after the dispatch completes. The dispatch itself (pure GPU compute) takes only ~5–10 ms. Giving the GPU 4× more concurrent SIMD groups helps hide latency within that 5–10 ms, but the savings are masked by the readback.

To see workgroup size effects, the readback must be eliminated (display/swap-chain architecture). Then the ~5–10 ms compute phase becomes the target, and occupancy effects would be measurable.

---

## Summary

Phase 5 built a complete GPU compute renderer using wgpu/WGSL in Rust, evolving from a stateless single-call design to a fully persistent `GpuScene` that writes only 128 bytes per frame.

Key findings:
1. **GPU wins rendering 10–17× over CPU parallel** because raymarching is embarrassingly parallel.
2. **CPU wins shadow computation by ~17×** because the running-max dependency chain is serial; GPU shader cores run at ~1/4 CPU clock with no advantage over the serial portion.
3. **Buffer beats texture** for this access pattern: raymarching's stripe-like spatial access doesn't benefit from the texture cache's 2D Morton-order layout, and the sampler unit adds latency without return.
4. **GpuContext amortizes ~80ms init cost**: essential for any multi-call use case.
5. **GpuScene reduces upload from 221 MB/frame to 128 bytes/frame**, delivering an additional 1.36× speedup over GPU_separate.
6. **The 85 MB readback is the hard floor**: ~88 ms of the 98 ms/frame budget. Eliminating it requires presenting frames directly to a display, bypassing the CPU entirely.
7. **Workgroup size (shape and thread count) has no effect** on this workload: all variants (64–256 threads, various shapes) within ±3%. The readback dominates; the compute phase is too short for occupancy tuning to matter.
