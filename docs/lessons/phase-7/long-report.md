# Phase 7 — Interactive Viewer: Comprehensive Student Textbook

**Hardware**: Apple M4 Max · 10 perf cores · 400 GB/s DRAM · 16 MB L2 · 48+ MB SLC
**Dataset**: USGS SRTM N47E011, 3601×3601
**Date range**: 2026-04-11 to 2026-04-18

---

## Part 1: The Swap-Chain Architecture

### 1.1 Why Swap Chains Exist

Phase 5 benchmarked GPU rendering at 10 fps on M4 for a 1600×533 scene. That number was almost entirely the CPU←→GPU readback: 85 MB of pixels read over PCIe (or unified memory bus) each frame took ~88ms. The GPU shader itself was doing ~0.2ms of work.

A **swap chain** eliminates readback entirely. The GPU writes pixels directly into a surface texture that the display compositor reads. The CPU never touches pixel data. This is how every real-time application — game engines, browsers, terminal emulators — renders.

The result: 10 fps → **470 fps** at 1600×533 on M4 (no-vsync). A 46× speedup that has nothing to do with shader quality — it only removes the readback tax.

### 1.2 Two Bottleneck Regimes

With readback gone, two distinct ceilings appear:

| step_m | fps | Bottleneck |
|---|---|---|
| `dx / 0.08` (huge steps, almost no work) | 470 | Command overhead floor (~2.1ms) |
| `dx / 10` (default) | 470 | Command overhead floor |
| `4.0m` (tiny steps, many iterations) | 85 | Shader compute |
| `8000×2667` resolution | 21 | Shader compute |

The command overhead floor (~2.1ms) is the fixed cost of building a command buffer, submitting it to the GPU scheduler, running a present operation, and returning. It exists even if the shader does zero work. You cannot get above ~470fps on M4 with wgpu's current command overhead.

Once shader work exceeds ~2ms, you're shader-bound. At step_m=4.0m and 1600×533, each frame has ~500 raymarching iterations per pixel × 0.85M pixels = 425M heightmap lookups.

### 1.3 winit 0.30 ApplicationHandler

wgpu surfaces require an `Arc<Window>` — the surface holds a reference to the window to guarantee the window outlives the surface. winit 0.30 switched from closure-based to trait-based event handling:

```rust
impl ApplicationHandler for Viewer {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // Create window + surface here — not in new()
        // Window is only valid after this event
    }
    fn window_event(&mut self, event_loop, window_id, event) { ... }
    fn device_event(&mut self, event_loop, device_id, event) { ... }
}
```

`resumed()` fires once after the OS gives the app a valid window. Surface and HUD must be created here because the surface format (Bgra8Unorm vs Rgba8Unorm) is only known after surface creation.

### 1.4 No-Readback Cross-System Numbers

| System | CPU fps | GPU+readback | GPU no-readback | Readback overhead |
|---|---|---|---|---|
| M4 Max | 14.8 | 46.2 | **477** | 10.3× |
| Win GTX 1650 | 0.87 | 10.8 | **260** | 24.1× |
| Mac i7 | 1.49 | 4.7 | **53** | 11.3× |
| Asus Pentium | 0.15 | 0.7 | **4.9** | 7.1× |

GTX 1650's 24.1× overhead ratio is the largest because PCIe readback is particularly expensive: 85 MB × 8 Gbit/s PCIe bandwidth ≈ 47ms/frame. True GPU compute on the 1650 is 3.8ms/frame — it was never slow, the readback hid it.

---

## Part 2: Camera and Window

### 2.1 WASD Mouse Camera

Camera state is `cam_pos: [f32; 3]`, `yaw: f32`, `pitch: f32`. Per-frame update:

1. `dt = last_frame.elapsed().as_secs_f32()`
2. Derive `forward_h` (horizontal forward, yaw-only) and `right_h`
3. Move `cam_pos` based on held keys at `speed = 500 m/s` (or 5000 m/s with Cmd/Alt held)
4. Derive look-at point: `cam_pos + dir` where `dir` includes pitch

Mouse look via `DeviceEvent::MouseMotion { delta: (dx, dy) }`. Sensitivity 0.001 rad/pixel. Two modes: hold-left-button or press Q for permanent immersive mode.

### 2.2 Window Resize and Buffer Alignment

wgpu requires `bytes_per_row` to be a multiple of 256 bytes (= 64 pixels at 4B/pixel). When the window is resized to an arbitrary width, rounding up is required:

```rust
let render_width = (new_width + 63) & !63;  // round up to 64-pixel boundary
```

Two separate values must be tracked:
- `render_width` — used for buffer allocation and `bytes_per_row` stride
- `width` — used for `Extent3d` in the copy (must not exceed surface texture width)

Using `render_width` for the copy extent caused "Copy would overrun destination texture" panics.

---

## Part 3: Rendering Quality

### 3.1 Bilinear Height Sampling

`textureLoad` snaps to the nearest integer texel. Each DEM cell is ~20m; at close range, one cell covers many screen pixels → blocky terrain. Fix: `textureSampleLevel(hm_tex, hm_sampler, uv, lod)` with a linear sampler.

Three API objects must all agree:
1. Sampler: `mag_filter: Linear, min_filter: Linear`
2. Bind group layout: `TextureSampleType::Float { filterable: true }`
3. Texture format: `R16Float` (not `R32Float` — `R32Float` requires `FLOAT32_FILTERABLE` device feature, not universal)

### 3.2 Sphere Tracing (Adaptive Step Size)

Fixed-step raymarching wastes iterations near terrain (tiny step, hit detected immediately) and misses sky (constant step through empty space). Adaptive step:

```wgsl
t_prev = t;
t += max((pos.z - h) * 0.2, cam.step_m);
```

`pos.z - h` is the height above terrain. When high above, the step is large (fast traversal). When close, falls back to `cam.step_m` (fine detail). The safety factor (0.2) is conservative — a steeper factor can overshoot narrow ridges.

Sky early exit prevents GPU timeout with small `step_m`:
```wgsl
if dir.z > 0.0 && pos.z > 4000.0 { break; }
```

Binary search refinement brackets the hit using `t_prev` (not `t - step_m`):
```wgsl
pos = binary_search_hit(t_prev, t, dir, 8);
```

### 3.3 Atmospheric Fog and Smooth Colors

Hard elevation color bands (if/else ladder) produce visible stripes in the distance where one screen pixel spans hundreds of DEM cells. Fix: `smoothstep` + `mix` transitions ±100m around thresholds. Atmospheric fog blends distant terrain to sky color:

```wgsl
let fog_t = smoothstep(15000.0, 60000.0, t);
final_color = mix(terrain_color, sky_color, fog_t);
```

### 3.4 C0 vs C1 Continuity — The Inescapable Artifact

Bilinear interpolation is **C0** (height is continuous at DEM grid lines) but **not C1** (the slope jumps at every grid boundary). These slope jumps create visible creases in the geometry that normals cannot fix.

Two attempted fixes, both reverted:
- GPU Gaussian smoothing on normals (5×5 kernel) — too soft, lost real terrain detail
- CPU Gaussian smoothing on heightmap — same problem

Conclusion: 20m/cell SRTM data has inherent resolution limits. The correct fix is bicubic interpolation or higher-resolution source data. Accepted as a hard floor.

---

## Part 4: HUD System

### 4.1 glyphon Integration

glyphon renders GPU-accelerated text via a glyph atlas texture. Key objects:
- `FontSystem` — CPU font loading + text shaping (layout)
- `SwashCache` — CPU glyph rasterisation
- `TextAtlas` — GPU texture holding rasterised glyphs
- `TextRenderer` — issues draw calls into a render pass
- `Viewport` — wraps surface resolution; updated each frame

The render pipeline requires `COPY_DST | RENDER_ATTACHMENT` on the surface texture. `COPY_DST` for the terrain blit; `RENDER_ATTACHMENT` for the text render pass.

Per-frame: prepare text areas → `begin_render_pass(LoadOp::Load)` → draw background quads → render text → drop render pass → finish encoder.

`LoadOp::Load` preserves the terrain pixels already in the texture. `LoadOp::Clear` would wipe them.

### 4.2 HudBackground (Semi-Transparent Quads)

Background behind text labels uses a custom wgsl shader with `SrcAlpha / OneMinusSrcAlpha` blending. Vertices are pixel-space coordinates converted to NDC in the vertex shader via a `[width, height]` uniform.

`build_vertices` returns `[f32; N]` for N triangles (each rect = 2 triangles = 6 vertices = 12 floats × 2 coords). The buffer is written via `queue.write_buffer` each frame when window size changes.

### 4.3 Sun/Season HUD Circles (SDF Shader)

A full-screen NDC quad lets the fragment shader see every pixel via `@builtin(position)`. Two clock-face circles drawn entirely with signed-distance fields:

- **Season circle**: 365-day face, Summer (day 172) at top. `day_angle = (day - 172).rem_euclid(365) / 365 * TAU`
- **Time circle**: 12-hour face, noon at top. `hour_angle = (hour % 12.0) / 12.0 * TAU`

Layers per circle (back to front): semi-transparent disc → coloured ring → tick marks at cardinal positions → yellow needle (SDF line segment) → white centre dot.

`discard` for pixels outside both circles and outside the background panel — ~99% of the full-screen quad is discarded at near-zero cost.

Drop shadows on text labels: render the same `glyphon::Buffer` twice — once at `(+1,+1)` with dark colour, then at correct position. Order in the `TextArea` slice = draw order.

---

## Part 5: Geographically Correct Sun

### 5.1 Spencer 1971 Solar Position

Replaced placeholder azimuth animation with physically accurate solar declination:

```rust
let decl = 23.45f32.to_radians() * ((360.0/365.0 * (day + 284) as f32).to_radians()).sin();
let h = (15.0 * (hour - 12.0)).to_radians();  // solar hour angle
let sin_el = lat.sin()*decl.sin() + lat.cos()*decl.cos()*h.cos();
```

`lat_rad` is derived from the tile's `origin_lat` and `dy_deg`. Shadow is only dispatched when `elevation > 0.0` (sun below horizon = no shadow computation).

### 5.2 Background Shadow Thread

Shadow computation (~1.5ms on M4) runs on a dedicated CPU thread while GPU renders the previous shadow:

```
Main thread:                    Worker thread:
try_recv() → got new mask?      recv() → blocking wait
  yes → scene.update_shadow()   while let Ok((az, el)) = rx.recv():
if !shadow_computing:             mask = compute_shadow_par(...)
  tx.try_send((az, el))          tx.send(mask)
  shadow_computing = true
```

`sync_channel(1)` bounds the queue to one pending job. `try_send` silently drops stale angles if the worker is still busy. When the main thread drops `shadow_tx`, `recv()` returns `Err` and the worker exits cleanly.

### 5.3 Soft Shadows (Penumbra)

Hard shadow masks produce aliasing at slow sun movement — individual pixels on the boundary flip one frame at a time. Soft shadow formula:

```rust
let margin = running_max - h_eff;  // metres above shadow horizon
data[i] = (1.0 - margin / penumbra_meters).max(0.0);
```

`penumbra_meters = 200.0` gives a 200m transition zone. The NEON version computes this with `vsub`/`vmul`/`vmax` — no branch.

---

## Part 6: Ambient Occlusion (6 Modes)

### 6.1 What AO Computes

Ambient occlusion measures what fraction of the upper hemisphere is visible from a terrain point. Valleys see less sky → darker. Ridges see more → brighter. This effect is independent of sun position.

AO mode is a `u32` in `CameraUniforms`, cycled with `/` (6 values via `rem_euclid(6)`). All 6 modes use a single shader with a uniform branch — zero wave divergence since every thread in a wave processes the same mode.

### 6.2 SSAO ×8 and ×16

16 precomputed hemisphere directions in a `const` array (4 rings at 15°/30°/45°/60°, 4 azimuths each). TBN frame from surface normal orients samples above the actual surface (not world-up, which would point into cliff faces).

```wgsl
let world_d = T * d.x + B * d.y + N * d.z;
let sample_pos = pos + world_d * 600.0;
open_factor += smoothstep(-50.0, 50.0, sample_pos.z - sample_h);
```

`smoothstep(-50, 50, z-h)` gives soft transitions ±50m around the surface. `ao_factor = open_factor / n_samples`.

### 6.3 HBAO ×4 and ×8

Marches outward in D horizontal directions, finding the maximum horizon elevation angle:

```wgsl
let cur_angle = atan2(sample_h - pos.z, probe_dist);
max_angle = max(max_angle, cur_angle);
// occlusion per direction:
1.0 - sin(max(0.0, max_angle))
```

`probe_dist` must start at 25.0 (not 0.0) — `atan2(h, 0)` = ±π/2 at the first step, causing maximum occlusion everywhere.

### 6.4 True Hemisphere (Precomputed)

`compute_ao_true_hemi(hm, 16, 5°, 200m)` sweeps 16 azimuths using the same DDA shadow function, accumulates the lit fraction, averages. 5° elevation threshold prevents distant mountains from fully blocking nearby open valleys. Result stored as `R8Unorm` texture (1 byte/texel, 13 MB, vs 52 MB for R32Float). One `textureSample` fetch at render time — effectively free.

True Hemi blend: `mix(1.0, raw_ao, 0.8)` — 80% of full effect prevents over-darkening.

### 6.5 Measured Numbers

**M4 Max**: No perceptible fps difference across all 6 modes. The raymarch loop (~500 samples/ray) dominates; SSAO adds 8–16 samples (1.6–3.2%), HBAO adds 96–192 — still negligible relative to the march.

**GTX 1650** (~50 fps baseline):

| Mode | Samples/pixel | fps drop |
|---|---|---|
| SSAO ×8 | 8 (fixed 600m radius) | 2–3 |
| SSAO ×16 | 16 | ~8 |
| HBAO ×4 | 96 (4 dirs × 24 steps, sweep to 600m) | ~20 |
| HBAO ×8 | 192 | ~25 |
| True Hemi | 1 (lookup) | 0 |

**Why the asymmetry?** M4's SLC (~48 MB+) holds the entire 13 MB R16Float heightmap. Every texture fetch hits cache regardless of access pattern. GTX 1650's smaller GDDR6 texture cache cannot hold the heightmap. HBAO's radial 600m sweep touches cache lines spread across a large UV area → most become GDDR6 fetches. SSAO's 8 point samples at fixed 600m offset stay closer together → more likely to stay in cache. This is the same cache-miss asymmetry that made diagonal shadow 2.4× slower than cardinal in Phase 3.

---

## Part 7: Level of Detail

### 7.1 Step-Size LOD

```wgsl
let lod_min_step = cam.step_m * (0.7 + t / 8000.0);
t += max((pos.z - h) * 0.2, lod_min_step);
```

At t=40km, `lod_min_step = step_m * 5.7` — far rays take fewer, coarser steps. The 0.7 base preserves near-camera quality.

**Ridge miss risk**: if `lod_min_step` exceeds ½ the narrowest feature (~100m for 200m ridges), rays jump over ridges without triggering a hit → pixel renders as sky. At D=500 this was visible as a "wave" of missed terrain. D=8000 is safe.

No fps gain on M4: extra iterations at distance are cache hits in the large SLC.

### 7.2 Mipmap LOD

7 mip levels generated at startup with **max filter** (2×2 → 1 = max of 4, not average). Max filter is conservative: peaks are never lowered, so rays still hit ridges at coarser mips.

```rust
mip_data.push(f16::from_f32(a.max(b).max(c).max(d)));
```

Shader: `let mip_lod = log2(1.0 + t / 15000.0)` — lod=1 at 15km (fog onset), lod=2 at 45km. `textureSampleLevel(hm_tex, hm_sampler, uv, mip_lod)`.

**Trilinear filtering**: `mipmap_filter: MipmapFilterMode::Linear` blends between mip levels based on the fractional part of `mip_lod`. Without it, there is a hard snap between mips → visible "traveling wave" as the camera moves. `FilterMode` and `MipmapFilterMode` are separate wgpu enums despite having the same variant names.

No fps gain on M4 for the same cache reason. Expected to matter more on GTX 1650 — coarser mips = fewer unique cache lines per ray = better GDDR6 hit rate.

---

## Part 8: Common Errors

| Error | Cause | Fix |
|---|---|---|
| `Option<Arc<Window>>: Into<SurfaceTarget>` | Stored `Option<Arc<Window>>`; surface target needs `Arc<Window>` | Extract `Arc<Window>` before assigning to `self.window` |
| `Adapter[Id] does not exist` panic | Created second `wgpu::Instance` in `resumed()`; surface and adapter from different instances | Store `pub instance` in `GpuContext`, reuse it |
| White screen (format mismatch) | Surface was `Bgra8UnormSrgb`; shader wrote RGBA | Select `Bgra8Unorm` explicitly; write BGRA in shader |
| White screen (sync race) | `dispatch_frame` submitted its own encoder; blit read output before GPU finished | Make `dispatch_frame` take `&mut CommandEncoder`, don't submit |
| White screen (no first frame) | `resumed()` never called `request_redraw()` | Add `request_redraw()` at end of `resumed()` |
| Q key toggled on press AND release | No `ElementState::Pressed` guard | Add `if event.state == ElementState::Pressed` |
| `COPY_BYTES_PER_ROW_ALIGNMENT` panic | Resized window had width not multiple of 64 px | Round up: `(w + 63) & !63` |
| Copy overrun texture | Used `render_width` (aligned) for `Extent3d` | Use actual `width` for `Extent3d`; `render_width` only for `bytes_per_row` |
| `R32Float` not filterable | `R32Float` requires `FLOAT32_FILTERABLE` device feature | Use `R16Float` (always filterable) |
| `wgpu::FilterMode` used for mipmap | Separate enum types in wgpu | Use `wgpu::MipmapFilterMode::Linear` |
| Out-of-bounds panic in DDA sweep | `.round()` maps values in `[N-0.5, N)` to `N` (out of bounds) | Replace all 22 occurrences with `.floor()` |
| HBAO full occlusion everywhere | `probe_dist` started at 0.0 → `atan2(h, 0) = ±π/2` | Start `probe_dist = 25.0` |
| Binary search wrong bracket | Used `t - step_m` as lower bracket with adaptive steps | Track `t_prev` before advancing; use `t_prev` as bracket |
| GPU normal smoothing too soft | 5×5 Gaussian removes DEM noise but also real ridgelines | Reverted — DEM resolution is a hard floor |

---

## Summary

- **Swap chain removes the readback tax entirely**: 10fps → 477fps (M4), 10.8fps → 260fps (GTX 1650). The GPU was always fast; readback hid it.
- **M4's large SLC makes cache-sensitive experiments invisible**: step-size LOD, mipmap LOD, all 5 AO modes show no fps difference on M4 because the 13 MB heightmap fits in cache. GTX 1650 shows large differences (HBAO ×4 = −20fps) because GDDR6 cache is smaller.
- **Readback overhead scales with PCIe bandwidth**: GTX 1650 suffers 24.1× overhead (47ms readback) vs M4's 10.3× (2.1ms). Discrete GPU is penalised for the very thing unified memory eliminates.
- **C1 discontinuity is a data resolution limit**: bilinear height is C0 but not C1 — slope jumps at every 20m DEM grid line. Smoothing blurs this but also destroys real terrain detail. Accepted as inherent.
- **True Hemisphere AO = sun shadow generalised**: the same DDA horizon sweep used for sun shadows, run in 16 directions and averaged, gives full-sphere ambient occlusion. Baked once at startup (16 × ~1.5ms ≈ 24ms), free at render time (one texture lookup).
