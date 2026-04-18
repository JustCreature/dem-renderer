# Phase 7 — Interactive Viewer: Reference Card

**Hardware**: Apple M4 Max · GTX 1650 · Mac i7 · Asus Pentium
**Dataset**: USGS SRTM 3601×3601. **Date**: 2026-04-11 to 2026-04-18

---

## 1. Swap-Chain — Eliminating Readback

wgpu Surface writes pixels directly to the display. CPU never touches pixel data. Phase 5's 10fps was 96% readback overhead (85 MB/frame). Without readback: M4 **477fps**, GTX 1650 **260fps**.

Two regimes: command overhead floor (~2.1ms = ~470fps cap) when shader work is trivial; shader-bound when step_m is small (e.g. 85fps at step_m=4.0m, 1600×533).

**No-readback cross-system:**

| System | CPU fps | GPU+rdback | GPU no-rdback | Overhead |
|---|---|---|---|---|
| M4 Max | 14.8 | 46.2 | 477 | 10.3× |
| Win GTX 1650 | 0.87 | 10.8 | 260 | 24.1× |
| Mac i7 | 1.49 | 4.7 | 53 | 11.3× |
| Asus Pentium | 0.15 | 0.7 | 4.9 | 7.1× |

---

## 2. winit 0.30 ApplicationHandler

Surface and HUD must be created in `resumed()` (not `new()`) — window handle and surface format are only valid after the OS grants a window. Store `instance` + `adapter` in `GpuContext` — creating a second `wgpu::Instance` in `resumed()` causes "Adapter[Id] does not exist" panic.

---

## 3. Render Quality

**Bilinear height**: `textureSampleLevel` instead of `textureLoad`. Requires: sampler `mag/min_filter: Linear` + BGL `filterable: true` + texture format `R16Float` (R32Float not universally filterable).

**Sphere tracing**: `t += max((pos.z - h) * 0.2, step_m)`. Adaptive step shrinks near terrain. Track `t_prev` for binary search bracket — `t - step_m` is wrong with variable steps. Sky early exit: `if dir.z > 0 && pos.z > 4000 { break }`.

**Smooth colors**: `smoothstep + mix` ±100m around elevation thresholds. **Fog**: `mix(terrain, sky, smoothstep(15000, 60000, t))`.

**C1 discontinuity**: bilinear is C0 (continuous height) not C1 (slope jumps at every 20m grid line). Gaussian smoothing on normals or heightmap blurs this but destroys real ridgelines. Reverted. Hard floor of the data resolution.

---

## 4. Buffer Alignment

wgpu requires `bytes_per_row` to be a multiple of 256 bytes (= 64 pixels). Track two values: `render_width = (width + 63) & !63` for buffer/stride; actual `width` for `Extent3d` copy (overrun if you use render_width there).

---

## 5. HUD System

**glyphon** (GPU text): `FontSystem` (CPU shaping) → `SwashCache` (CPU rasterise) → `TextAtlas` (GPU atlas) → `TextRenderer` (draw calls). Surface needs `COPY_DST | RENDER_ATTACHMENT`. Use `LoadOp::Load` to preserve terrain. Drop-shadow: render same buffer twice (offset + dark colour, then real position).

**SDF circles** (`shader_sun_hud.wgsl`): full-screen NDC quad, all math in fragment shader. `discard` for pixels outside circles/panel. Season circle: `(day-172).rem_euclid(365) / 365 * TAU`. Time: `(hour % 12.0) / 12.0 * TAU`.

---

## 6. Geographically Correct Sun

Spencer 1971 declination: `δ = 23.45° × sin(360°/365 × (day+284))`. Solar hour angle: `H = 15° × (hour-12)`. Combined with latitude → elevation + azimuth. Shadow dispatched only when elevation > 0.

Background shadow thread: `sync_channel(1)`, `try_send` discards stale angles. Worker exits when sender dropped. On M4: shadow ~1.5ms, frame ~2.1ms → shadow always ready next frame. On GTX 1650 at 15fps → possible stale shadow frame when camera moves fast.

**Soft shadows**: `(1.0 - margin / penumbra_meters).max(0.0)`, default 200m. Eliminates per-pixel boundary aliasing at slow sun movement.

---

## 7. Ambient Occlusion (6 Modes, `/` Key)

| Mode | ao_mode | Cost M4 | Cost GTX 1650 |
|---|---|---|---|
| Off | 0 | baseline | baseline (~50fps) |
| SSAO ×8 | 1 | ~0 | −2–3fps |
| SSAO ×16 | 2 | ~0 | −8fps |
| HBAO ×4 | 3 | ~0 | −20fps |
| HBAO ×8 | 4 | ~0 | −25fps |
| True Hemi | 5 | ~0 | 0fps |

**Why M4 is flat**: 13 MB heightmap fits in M4 SLC (~48 MB+) → every fetch hits cache regardless of access pattern.

**Why GTX 1650 degrades on HBAO**: HBAO sweeps 24 steps × 25m per direction = 600m radius. 96–192 samples spread across a large UV area → most miss the GDDR6 texture cache. SSAO's 8 fixed-offset samples cluster together → more cache-friendly.

**SSAO**: TBN frame from surface normal (avoid world-up on cliffs — `up_ref` swap when `|N.y| > 0.9`). 16-entry const kernel (4 rings × 4 dirs). `smoothstep(-50, 50, z-h)` for soft occlusion.

**HBAO**: `atan2(h_sample - pos.z, probe_dist)`, `probe_dist` starts at 25.0 (not 0 — avoids singularity). `1.0 - sin(max_angle)` → natural [0,1] range.

**True Hemi**: run shadow DDA in 16 azimuths, average lit fractions. `R8Unorm` (13 MB), bilinear sampler. Blend: `mix(1.0, raw_ao, 0.8)` to prevent over-darkening. Bug found: `.round()` in DDA index → out-of-bounds at grid edges; fix: `.floor()`.

---

## 8. Level of Detail

**Step LOD**: `lod_min_step = step_m * (0.7 + t / 8000.0)`. Safe range: D ≥ 5000 (prevents ridge-miss artifact where step > ½ narrowest feature). No fps gain on M4 (cache-bound).

**Mipmap LOD**: 7 levels, **max filter** (not average — average lowers peaks → ray misses). `mip_lod = log2(1 + t/15000)`. Trilinear: `mipmap_filter: MipmapFilterMode::Linear` (not `FilterMode` — different enum). Without it: traveling wave artifact at mip boundaries.

---

## 9. Common Errors

| Error | Cause | Fix |
|---|---|---|
| Adapter panic | Second `wgpu::Instance` in `resumed()` | Store instance in `GpuContext`, reuse |
| White screen | `Bgra8UnormSrgb` format mismatch | Select `Bgra8Unorm`, write BGRA in shader |
| White screen | `dispatch_frame` submitted its own encoder | Pass `&mut CommandEncoder` instead |
| `COPY_BYTES_PER_ROW_ALIGNMENT` panic | Unaligned resize width | `(w + 63) & !63` |
| Copy overrun | `render_width` used for Extent3d | Use `width` for Extent3d |
| R32Float not filterable | Requires device feature | Use R16Float |
| Mipmap wave artifact | `FilterMode` used instead of `MipmapFilterMode` | `MipmapFilterMode::Linear` |
| DDA out-of-bounds | `.round()` at boundary | `.floor()` (22 occurrences) |
| HBAO full occlusion | `probe_dist = 0.0` → `atan2(h,0) = ±π/2` | Start `probe_dist = 25.0` |
| Wrong binary search bracket | `t - step_m` with adaptive step | Track `t_prev` |
