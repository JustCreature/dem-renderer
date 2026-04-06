# Phase 5 — GPU Renderer: Reference Document

## 1. GPU Execution Model

**SIMT (Single Instruction, Multiple Threads)**: all threads in a SIMD group execute the same instruction simultaneously on different data. Apple Silicon SIMD group = 32 threads. NVIDIA warp = 32. AMD wavefront = 64.

**Workgroup**: a group of threads that share scratchpad memory. We used 8×8 = 64 threads = 2 SIMD groups. The workgroup is the unit of scheduling on a GPU core.

**Latency hiding**: GPU hides memory latency (400–800 cycles) by switching to another ready SIMD group when one stalls. Requires many warps in flight (occupancy).

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

Fixed initialization overhead: ~50–80ms. Dominates at small images. Amortize in real-time renderers by reusing device/pipeline across frames.

---

## 3. Buffer Types and Usages

| Usage | Purpose |
|---|---|
| `STORAGE \| COPY_SRC` | GPU output buffer (shader writes, then copies out) |
| `MAP_READ \| COPY_DST` | Readback buffer (CPU reads after GPU copy) |
| `UNIFORM \| COPY_DST` | Camera parameters (small, cached, read-only in shader) |
| `STORAGE` (read-only) | Heightmap, normals, shadow mask |

Cannot map a `STORAGE` buffer for CPU reading. Must `copy_buffer_to_buffer` to a `MAP_READ` buffer first.

`create_buffer_init`: creates + uploads data. `create_buffer`: creates empty buffer.

---

## 4. Uniform Buffer Layout (std140)

- `f32`/`u32`/`i32`: 4 bytes, 4-byte aligned
- `vec3<f32>`: **16 bytes** (12 data + 4 implicit pad) ← critical
- Struct total size must be multiple of 16 bytes

Rust side: `#[repr(C)]` (field order preserved), `bytemuck::Pod` + `bytemuck::Zeroable` (safe cast to `&[u8]`). Explicit `_pad` fields after every `[f32; 3]`.

Debugging: wgpu error reports exact expected size. Count Rust struct bytes and add pads until they match.

Our `CameraUniforms`: 144 bytes (9 × 16). Fields: origin, forward, right, up, sun_dir (each `[f32;3]` + pad), then half_w, half_h, img_width, img_height, hm_cols, hm_rows, dx_meters, dy_meters, step_m, t_max, 6 pad floats.

---

## 5. Bind Group Model

Two objects must match each other and the shader:

1. **BindGroupLayout**: declares types at each binding number (schema)
2. **BindGroup**: assigns real resources to binding numbers (instance)

All three must agree: `@group(0) @binding(N)` in shader ↔ layout entry with `binding: N` ↔ bind group entry with `binding: N`.

Buffer binding type: `wgpu::BindingType::Buffer { ty: BufferBindingType::Storage { read_only: ... }, ... }`
Texture binding type: `wgpu::BindingType::Texture { sample_type: Float { filterable: false }, view_dimension: D2, multisampled: false }`
Sampler binding type: `wgpu::BindingType::Sampler(SamplerBindingType::NonFiltering)`

---

## 6. WGSL Shader Structure

```wgsl
@group(0) @binding(0) var<uniform> cam: CameraUniforms;
@group(0) @binding(1) var<storage, read> hm: array<f32>;  // buffer version
// OR
@group(0) @binding(1) var hm_tex: texture_2d<f32>;        // texture version
@group(0) @binding(2) var hm_sampler: sampler;

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) { ... }
```

**Dispatch**: `pass.dispatch_workgroups((width+7)/8, (height+7)/8, 1)` — ceiling division ensures full coverage.

**Ray direction**:
```wgsl
let u = (f32(gid.x) + 0.5) / f32(cam.img_width);
let ndc_x = -(2.0 * u - 1.0);   // flip horizontal
let ndc_y = 1.0 - 2.0 * v;       // flip vertical
let dir = normalize(cam.forward + cam.right * ndc_x * cam.half_w + cam.up * ndc_y * cam.half_h);
```

**Heightmap read (buffer)**: `hm[u32(row) * cam.hm_cols + u32(col)]`
**Heightmap read (texture)**: `textureLoad(hm_tex, vec2<i32>(col, row), 0).r`

`textureLoad`: integer coords, exact texel, no interpolation. Returns `vec4<f32>` — use `.r` for first channel.

---

## 7. Textures

**Texel**: texture element — GPU equivalent of a pixel in a data buffer.

**`R32Float`**: one f32 per texel. `R` = 1 channel, `32` = bit depth, `Float` = type.

**Key parameters**:
- `mip_level_count: 1` — no mipmaps (we want exact values, not averaged)
- `sample_count: 1` — no MSAA
- `dimension: D2` — 2D grid

**Upload**:
```rust
queue.write_texture(
    hm_texture.as_image_copy(),
    bytemuck::cast_slice(&hm_f32),
    wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(cols * 4), rows_per_image: None },
    wgpu::Extent3d { width: cols, height: rows, depth_or_array_layers: 1 },
);
```
`bytes_per_row = cols * 4` because one f32 = 4 bytes, no row padding.

**Texture cache**: dedicated 2D-aware L1 cache (~4–16 KB per shader array). Stores data in internal Morton-order tiling so 2D neighborhoods fit in one cache line. Storage buffer cache is linear — 2D-adjacent rows are `cols * 4 ≈ 14 KB` apart in address space.

---

## 8. Texture vs Buffer Experiment

**Setup**: 8000×2667 = 21.3M pixels, M4 Max, varying step_m.

| step_m divisor | Buffer | Texture | Abs saving | % advantage |
|---|---|---|---|---|
| /0.8 (26m) | 0.13s | 0.11s | 0.02s | 18% |
| /2.0 (15m) | 0.23s | 0.19s | 0.04s | 17% |
| /4.0 (7.5m) | 0.36s | 0.31s | 0.05s | 14% |
| /8.0 (3.7m) | 0.64s | 0.59s | 0.05s | 8% |

**Finding**: absolute saving plateaus at ~0.05s regardless of step count. Percentage advantage shrinks.

**Conclusion**: texture cache benefit comes from **cross-thread spatial locality** (64 threads in a workgroup start near each other → nearby terrain → 2D cache hit), not from intra-ray temporal reuse. As rays diverge deeper in the scene, neither cache helps. The ~0.05s is the fixed cost of the early traversal phase. Both variants scale linearly with step count → compute-bound, not bandwidth-limited.

---

## 9. GPU vs CPU Performance

At 8000×2667 (21.3M pixels), step_m = dx/0.8:

| Renderer | Time | Pixels/sec | Speedup vs CPU parallel |
|---|---|---|---|
| CPU scalar | ~8.5s | ~2.5 M/s | — |
| CPU parallel (10c) | 1.03s | ~20.7 M/s | 1× |
| GPU buffer | 0.13s | ~164 M/s | 7.9× |
| GPU texture | 0.11s | ~194 M/s | 9.4× |

**Crossover**: at 2000×900 (1.8M pixels), GPU ≈ CPU parallel (~0.08s each). wgpu init overhead (~50–80ms) dominates below ~3–5M pixels.

---

## 10. FOV and Panoramic Images

`half_w = tan(hfov/2)`, `half_h = half_w / aspect`. Both stored in uniform.

For panoramic 8000×2667 (aspect ≈ 3:1), hfov = 100°:
- `vfov = 2 * atan(tan(50°) / 3) ≈ 40°`

Too wide (hfov > 150°) causes severe distortion at ray edges — nearly horizontal rays that never hit terrain. Typical panoramic: hfov 90°–120°.

---

## 11. Common Errors

| Error | Cause | Fix |
|---|---|---|
| All black | `hm.data` is `Vec<i16>`, uploaded as raw bytes | Convert: `hm.data.iter().map(\|&v\| v as f32)` |
| Banded stripes | `RgbImage` (3 bytes) with RGBA buffer (4 bytes) | Use `RgbaImage::from_raw` |
| Wrong terrain shape | `pos.y / dx_meters` instead of `dy_meters` | Add separate `dy_meters` uniform |
| Uniform size mismatch | `vec3` needs 4-byte pad; struct not multiple of 16 | Add `_pad` fields; check `size_of` |
| Arc artifacts | No binary search refinement | Add `binary_search_hit` between step N-1 and N |
| Panoramic mismatch | `cam` not rebuilt after changing `pic_width` | Declare `cam` as `let mut`; rebuild with new aspect |
| Entry point not found | Empty shader file | Verify file has `@compute fn main(...)` |
