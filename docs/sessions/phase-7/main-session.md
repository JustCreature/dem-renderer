# Phase 7 Session Log

## 2026-04-11

### Objective
Implement an interactive viewer (`--view` flag) that renders terrain directly to a wgpu Surface (swap chain) without PCIe readback, to measure true GPU shader throughput.

### What was built

**`src/viewer.rs`** — new file, complete interactive viewer:
- `Viewer` struct: `scene`, `window`, `surface`, `surface_config`, `width`, `height`, `cam_x`, `cam_y`, FPS counter fields
- `impl ApplicationHandler for Viewer`: `resumed()` creates window + surface + selects `Bgra8Unorm` format; `window_event()` handles `CloseRequested` and `RedrawRequested`
- `RedrawRequested` pattern: single encoder → `dispatch_frame` (records compute pass) → `copy_buffer_to_texture` → `submit` → `present` → `request_redraw` (render loop)
- `fn run(tile_path, width, height)` — public entry point
- `fn prepare_scene(...)` — loads heightmap, computes CPU shadow (NEON parallel), creates GpuContext + GpuScene; returns `(GpuScene, cam_x, cam_y)`
- `fn downsample(hm: &Heightmap, factor: usize) -> Heightmap` — takes every Nth sample; scales `dx_meters *= factor`, `dy_meters *= factor`

**`src/main.rs`** — added `mod viewer;` and `--view` branch at top of `main()`

**`crates/render_gpu/src/scene.rs`** — added:
- `pub fn dispatch_frame(&self, encoder: &mut wgpu::CommandEncoder, ...)` — records compute pass into caller's encoder, no submit
- `pub fn get_output_buffer() -> &wgpu::Buffer`
- `pub fn get_gpu_ctx() -> &GpuContext`
- `pub fn get_dx_meters() -> f32`
- `pub fn get_dy_meters() -> f32`

**`crates/render_gpu/src/context.rs`** — added `pub instance: wgpu::Instance` and `pub adapter: wgpu::Adapter` fields

**`crates/render_gpu/src/shader_texture.wgsl`** — changed pixel packing to BGRA byte order for macOS Metal `Bgra8Unorm` surface

**`Cargo.toml`** (root) — added `wgpu = "29.0.1"` and `winit = "0.30"` dependencies

### Key bugs fixed during implementation
1. `Option<Arc<Window>>: Into<SurfaceTarget>` — fixed by extracting `Arc<Window>` before assigning to `self.window`
2. `Adapter[Id] does not exist` panic — caused by creating a second `wgpu::Instance` in `resumed()`; fixed by storing `pub instance` in `GpuContext` and reusing it
3. White screen (format mismatch) — surface was `Bgra8UnormSrgb`; fixed by selecting `Bgra8Unorm` + writing BGRA byte order in shader
4. White screen (sync race) — `dispatch_frame` was submitting its own encoder; blit then read output_buf before GPU finished; fixed by making `dispatch_frame` take `&mut CommandEncoder` and not submit
5. White screen (no first frame) — `resumed()` never called `request_redraw()`; fixed by adding it at end of `resumed()`
6. Camera world position bug — `cam_col * scene.get_dx_meters()` where `dx_meters` scales with downsample factor; fixed by computing `cam_x/cam_y` from `hm_raw.dx_meters` before downsampling and storing in `Viewer` struct

### Concepts covered
- **winit 0.30 `ApplicationHandler` trait**: `resumed()` for window creation, `window_event()` for event handling; replaces old closure-based API
- **`wgpu::Surface<'static>`**: requires `Arc<Window>` so surface can hold window reference for its lifetime
- **wgpu Instance sharing**: surface and adapter must come from the same `Instance`; mixing instances causes panic
- **Single encoder pattern**: compute dispatch + buffer-to-texture copy in one encoder guarantees ordering without explicit sync
- **`wgpu::CurrentSurfaceTexture` enum** (wgpu 29): `Success(t) | Suboptimal(t) | Timeout | ...` — not a `Result`
- **`PresentMode::Fifo` vs `Immediate`**: Fifo = vsync-capped; Immediate = uncapped, shows true GPU throughput
- **BGRA vs RGBA**: Metal surfaces use `Bgra8Unorm`; shader must write `b | (g<<8) | (r<<16) | (a<<24)`

### Measured numbers

#### Viewer fps (swap-chain, no readback)
- `bench_fps` baseline (Phase 5): 10fps — almost entirely 85 MB readback (~88ms)
- Viewer with `PresentMode::Immediate`: **470fps** (2.1ms/frame) — 46× faster, proves readback was the bottleneck

#### Identifying the bottleneck regimes (1600×533)
| step_m | fps | Bottleneck |
|---|---|---|
| `dx / 0.08` (large steps) | 470 | Command overhead floor |
| `dx / 0.8` (default) | 470 | Command overhead floor |
| `4.0m` (fixed) | 85 | Shader compute |
| `dx / 100.8` (tiny steps) | 17 | Shader compute |

- Decreasing step_m (more work) → fps drops proportionally (shader-bound)
- Increasing step_m (less work) → fps hits 470fps ceiling (overhead-bound)
- Fixed per-frame overhead ≈ 2.1ms (command buffer submission + GPU scheduling + present)

#### Texture cache experiment (fixed step_m = 4.0m)
Goal: isolate GPU texture cache pressure by varying heightmap resolution

| Resolution | width×height | fps |
|---|---|---|
| 1600×533, factor=1 (3601×3601 hm) | 0.85 Mpix | 85 |
| 1600×533, factor=2 (1800×1800 hm) | 0.85 Mpix | 85 |
| 1600×533, factor=4 (900×900 hm) | 0.85 Mpix | 85 |
| 8000×2667, factor=1 (3601×3601 hm) | 21.3 Mpix | 3 |
| 8000×2667, factor=4 (900×900 hm) | 21.3 Mpix | 3 |

**Result: no fps difference across any factor.** Bottleneck is compute throughput (loop iterations), not texture cache.

### Lessons
1. **Swap-chain eliminates the readback tax entirely**: 10fps → 470fps is not an improvement in shader performance — it removes PCIe/readback overhead. Shader work itself was always fast (~0.2ms at 0.85 Mpix with default step_m).
2. **Two distinct bottleneck regimes**: command overhead floor (~2.1ms, ~470fps) and shader compute. The transition is controlled by step_m.
3. **Texture cache experiment is bottleneck-dependent**: at fixed step_m, increasing heightmap resolution (factor=1 vs factor=4) has no effect because the bottleneck is compute throughput (loop iterations per ray), not texture fetch bandwidth. On M4's unified memory with large GPU L2, the working set fits in cache regardless of resolution.
4. **Discrete GPU would differ**: GTX1650 (Phase 6) has a much smaller texture cache backed by GDDR6. Factor=1 (26MB) vs factor=4 (1.6MB) would show measurable difference there.
5. **Sky rays are expensive with small step_m**: with step_m=4.0m and t_max=200,000m, sky-pointing rays iterate 50,000 steps. At 8000×2667, this dominates frame time (333ms/frame, 3fps).

---

## 2026-04-11 (session 2)

### What was covered

**Viewer reverted to clean state (no downsample).** Measured true GPU compute at 8000×2667:
21fps (47ms/frame). Corrected the Phase 5 estimate — readback and compute were overlapping
in the bench measurement; real compute was always ~47ms, not ~10ms.

**vsync flag implemented by user:**
- `--vsync` CLI arg → `PresentMode::Fifo`; default = `PresentMode::Immediate`
- Fallback to Fifo if Immediate not supported (with console warning)
- Result: vsync on = 100fps (display-capped), vsync off = 470fps at 1600×533

**Camera movement implemented (all 4 steps):**

Step 1 — `Viewer` struct extended:
- `cam_pos: [f32; 3]`, `yaw: f32`, `pitch: f32`
- `keys_held: HashSet<KeyCode>`
- `fps_timer: Instant` (separate from `last_frame` to fix fps display)

Step 2 — `WindowEvent::KeyboardInput`: insert/remove from `keys_held` on press/release.
Uses `if let PhysicalKey::Code(kc) = event.physical_key` to skip unidentified keys.

Step 3 — `device_event` override: `DeviceEvent::MouseMotion { delta: (dx, dy) }` →
update yaw/pitch with sensitivity 0.001 rad/pixel; clamp pitch to ±1.57.

Step 4 — `RedrawRequested` now:
- Computes `dt = last_frame.elapsed()` at top of frame
- Derives `forward_h` and `right_h` from yaw (horizontal only, no pitch in movement)
- Updates `cam_pos` for WASD + Space/ShiftLeft held keys (speed = 500 m/s)
- Derives full `fwd` vector (with pitch) for look_at
- Passes `self.cam_pos` and `look_at` to `dispatch_frame`

**Mouse interaction modes:**
- Normal mode: hold left mouse button to look (cursor locked while held, restored on release)
- Immersive mode: press Q to toggle — cursor locked permanently, mouse always controls look
- Bug fixed: Q was toggling on both press and release (net no-op); fixed with
  `event.state == ElementState::Pressed` guard
- Bug fixed: in immersive mode `mouse_look` stayed false; fixed by checking
  `self.mouse_look || self.immersive_mode` in `device_event`

**Quality improvement discussion — identified 3 priorities (saved to viewer-plan.md):**
1. `textureSampleLevel` instead of `textureLoad` — bilinear height interpolation, eliminates blocky tile edges
2. `smoothstep`/`mix` for elevation color bands — removes hard color stripes
3. Normal interpolation — bilinear blend of 4 neighbours, smooths shading

Root cause of "big tiles": `textureLoad` snaps to nearest integer texel; each DEM cell
is ~21m; at close range one cell covers many screen pixels → hard edges visible.
Sampler is already bound at `@binding(2)` but unused — Priority 1 is a ~10-line shader change.

**viewer-plan.md updated** with: actual measured numbers, full current implementation state,
HUD plan (glyphon), quality improvement plan with WGSL code snippets.

### Open items for this phase
- CameraState + InputState: WASD/mouse camera movement (not yet started)
- GPU timestamp queries to measure per-pass GPU time without frame overhead (not yet started)
- Remove debug prints (`if self.frame_count == 0 { println!(...) }`) from viewer.rs before phase finalisation
- Fix step_m back to `scene.get_dx_meters() / 0.8` (default) after texture cache experiment

---

## 2026-04-11 (session 3)

### What was covered

**Fixed `wgpu::PollType::Wait` struct variant error** in `src/benchmarks/phase6.rs`.
wgpu 29 changed `PollType::Wait` from a unit variant to a struct variant requiring
`{ submission_index: Option<T>, timeout: Option<T> }`. Both occurrences replaced:
```rust
ctx.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None })
```
Build now clean (warnings only, no errors).

**Ran `bench_fps` with the new GPU-scene-no-readback variant on all 4 systems.**
Results saved to `docs/benchmark_results/report_1/no_readbacks_fps/`.

**Extracted no-readback data to `fps_no_readback.csv`** — new file with 8 rows:
CPU fps, GPU combined (rdback) fps, GPU scene (no rdback) fps, GPU speedup vs CPU,
readback overhead ratio — for all 4 systems.

**Updated `report_1.md`** — section 6 split into two sub-tables:
- Phase 6 baseline (with readback)
- Phase 7 no-readback table with readback overhead column
- Conclusion 2 updated with actual measured numbers (Win GTX ~50fps prediction corrected to 260fps)

**Updated `report_1.html`**:
- FPS tab: no-readback section moved to top (primary), with-readback baseline below
- New stat boxes: 477/260/52.9/4.9 fps no-rdback for all systems
- New chart `chart_fps_no_rdback` — 3-series log-scale bar (CPU / GPU+rdback / GPU no-rdback)
- Overview chart updated: 3 datasets + log scale to accommodate 477fps range
- Overview headline stat updated: 46.4 → 477 fps (no-rdback)
- 3 insight boxes: readback 24.1× overhead on Win GTX, M4 477fps explanation,
  original prediction correction

### Key numbers measured (all systems, 1600×533, 30-frame pan)

| System | CPU fps | GPU+rdback fps | GPU no-rdback fps | Readback overhead |
|---|---|---|---|---|
| M4 Mac | 14.8 | 46.2 | **476.9** | 10.3× |
| Win GTX 1650 | 0.87 | 10.8 | **260.1** | 24.1× |
| Mac i7 | 1.49 | 4.7 | **52.9** | 11.3× |
| Asus Pentium | 0.15 | 0.7 | **4.9** | 7.1× |

**Key insight:** Win GTX 1650 was spending 96% of each frame waiting for CPU readback.
True GPU compute = 3.8 ms/frame. Phase 6 "~50 fps prediction" was 5× wrong — actual
no-readback fps is 260. The GPU was always fast; readback hid it completely.

### Open items for this phase
- GPU timestamp queries (measure per-pass GPU time without frame overhead)
- Picture quality: bilinear height sampling (`textureSampleLevel`), smooth color bands, normal interpolation
- HUD text overlay (`glyphon`)
- Window resize handling (`WindowEvent::Resized`)

---

## 2026-04-11 (session 4)

### What was covered

**Bilinear height sampling via `textureSampleLevel`** — replaced both `textureLoad` call sites
in `shader_texture.wgsl` with `textureSampleLevel(hm_tex, hm_sampler, uv, 0.0).r`.
UV computed as `(pos.x / dx + 0.5) / hm_cols` (half-texel offset for centre alignment).

**Three-layer fix required to enable bilinear filtering:**
1. Shader: `textureLoad` → `textureSampleLevel` + UV coordinates
2. Rust sampler: `SamplerDescriptor::default()` (Nearest) → `mag_filter/min_filter: Linear`
3. Bind group layout: `TextureSampleType::Float { filterable: false }` → `filterable: true`
   (both `normals_init_bgl` and `render_bgl` in `scene.rs`)
4. Texture format: `R32Float` → `R16Float` — `R32Float` is not filterable on all platforms
   without `FLOAT32_FILTERABLE` device feature; `R16Float` is always filterable.
   Required adding `half = { version = "2", features = ["bytemuck"] }` to `render_gpu/Cargo.toml`
   and converting `Vec<f32>` → `Vec<f16>` before upload; `bytes_per_row` halved to `cols * 2`.

**Bilinear normal interpolation** — replaced single `shadow[idx]` / `nx[idx]` lookups with
manual 4-texel bilinear blend using fractional offsets `fx`/`fy` derived from `pos.x`/`pos.y`.
`normalize(mix(mix(n00, n10, fx), mix(n01, n11, fx), fy))` — normalize after blend is required
because lerping two unit vectors produces a shorter-than-unit vector.

**Bilinear shadow interpolation** — same pattern, scalar blend:
`mix(mix(s00, s10, fx), mix(s01, s11, fx), fy)` — gives soft shadow edges instead of hard
square boundaries.

**Smooth elevation color bands** — replaced hard `if/else` ladder with `smoothstep` + `mix`:
- 4 base colors: green, light_green, rock, snow
- 3 transition zones ±100m around original thresholds (1900, 2100, 2700m)
- `mix(mix(mix(green, light_green, t1), rock, t2), snow, t3)`

**Atmospheric fog** — blends distant terrain toward sky color based on march distance `t`:
- `smoothstep(15000.0, 60000.0, t)` — fog starts at 15km, fully haze at 60km
- Applied after brightness, before pixel pack: `mix(f32(r), sky.x, fog_t)`
- Hides hard color band lines at distance (root cause: at distance each screen pixel spans
  many DEM cells, so a 200m smoothstep zone is sub-pixel and invisible without fog)

### Key concepts

- **`textureSampleLevel` requires a filtering sampler AND a filterable texture format** —
  three separate API objects must all agree: sampler `FilterMode`, bind group layout
  `filterable` flag, and the texture format itself. Missing any one gives a wgpu validation error.
- **`R32Float` is not universally filterable** — requires `FLOAT32_FILTERABLE` device feature
  (not available on all hardware). `R16Float` is always filterable; sufficient for terrain heights.
- **Bilinear normal interpolation**: lerp 4 neighbours, then normalize — lerp alone shortens
  the vector.
- **`mix(a, b, t)`** = lerp. Works element-wise on `vec3`. `smoothstep(e0, e1, x)` gives
  cubic ease 0→1 across [e0, e1].
- **Fog as distance-based blend** masks sub-pixel color transitions at distance more effectively
  than noise/threshold variation.

### Open items for this phase
- GPU timestamp queries (measure per-pass GPU time without frame overhead)
- HUD text overlay (`glyphon`)
- Window resize handling (`WindowEvent::Resized`)

---

## 2026-04-12 (session 5)

### What was covered

**Updated `shader_buffer.wgsl` and CPU shading to match `shader_texture.wgsl` quality.**

`shader_buffer.wgsl` — full rewrite:
- Added `sample_hm(x, y)` helper: manual bilinear interpolation from storage buffer
  (storage buffers have no sampler; must compute `col_f/row_f`, lerp 4 neighbours manually)
- Bilinear normal interpolation (same `mix`/`normalize` pattern as texture shader)
- Bilinear shadow interpolation
- Smooth elevation color bands (`smoothstep` + `mix`, 4 colors)
- Atmospheric fog (`smoothstep(15000, 60000, t)` blend to sky color)
- Fixed pixel packing: buffer shader uses `Rgba8Unorm` output (non-display path), so RGBA not BGRA

`crates/render_cpu/src/lib.rs` — `shade()` updated:
- Bilinear normal lookup: `col0 = min(col_f as usize, hm.cols - 2)`, `fx/fy` fractional offsets,
  4-neighbour blend + normalize
- Bilinear shadow lookup: same `blerp` closure, 4-neighbour blend
- Smooth elevation color bands: `smoothstep` closure + `lerp3`, matching GPU shader thresholds
- CPU renders are PNG files, no camera distance available → fog not added (would need `t` passed in)

**Window resize handling** — implemented `WindowEvent::Resized` in `src/viewer.rs`:
- Added `render_width: u32` field to `Viewer` struct — aligned render width (actual width rounded
  up to next multiple of 64)
- `Resized` handler: guards against zero-size (minimize), updates `self.width`/`height`,
  recomputes `render_width = (new_size.width + 63) & !63`, reconfigures surface,
  calls `scene.resize(render_width, height)`
- `bytes_per_row: Some(self.render_width * 4)` — stride uses aligned width
- `Extent3d { width: self.width, ... }` — copy extent uses actual window width
- Two bugs fixed during implementation:
  1. `COPY_BYTES_PER_ROW_ALIGNMENT` panic: `wgpu` requires `bytes_per_row` to be a multiple of 256
     (= 64 pixels × 4 bytes). Default 1600px window was fine; odd resize targets were not.
     Fix: round up to 64-pixel boundary.
  2. "Copy would overrun destination texture": aligned render_width (e.g. 1664) used as copy
     extent but surface texture is only actual width (e.g. 1610) wide. Fix: use `self.width`
     (actual) for `Extent3d`, use `self.render_width` only for `bytes_per_row` stride.

**`GpuScene::resize()` implemented** in `crates/render_gpu/src/scene.rs`:
- Added `render_bgl: wgpu::BindGroupLayout` field to `GpuScene` struct (stored to rebuild bind group)
- `pub fn resize(&mut self, width: u32, height: u32)` reallocates:
  - `output_buf`: `STORAGE | COPY_SRC`, size `width * height * 4`
  - `readback_buf`: `MAP_READ | COPY_DST`, size `width * height * 4`
  - Rebuilds `render_bg` against `self.render_bgl` with all 8 bindings

**Bind group / bind group layout explanation covered:**
- **BGL** = pipeline blueprint: describes *what type* of resource is at each binding slot.
  Created once, lives in the shader pipeline. Read-only.
- **BG** = live snapshot: holds the *actual GPU buffer/texture handles* for one dispatch.
  Must be recreated whenever the output buffer changes size (new allocation = new GPU address).
  The shader sees whatever BG is bound at dispatch time.
- When `resize()` reallocates `output_buf`, the old BG still points to the old (now freed) buffer.
  Rebuilding BG with the new buffer handle makes the shader write to the right memory.
- Texture pipeline: BGL declares format+sample type; texture is pre-allocated at `new()` and
  never resized (heightmap is fixed), so texture BG never needs rebuilding.

**Sphere tracing (adaptive step size)** — implemented in `shader_texture.wgsl`:

1. Sky ray early exit:
   ```wgsl
   if dir.z > 0.0 && pos.z > 4000.0 { break; }
   ```
   Prevents GPU timeout with small `step_m` — sky-pointing rays above 4km cannot hit terrain.

2. Adaptive step (sphere tracing):
   ```wgsl
   t_prev = t;
   t += max((pos.z - h) * 0.5, cam.step_m);
   pos = cam.origin + dir * t;
   ```
   Step scales with height above terrain. Safety factor 0.5 is conservative for steep slopes.
   `cam.step_m` is the minimum step to prevent stalling near-surface.

3. Binary search bracket fixed:
   ```wgsl
   pos = binary_search_hit(t_prev, t, dir, 8);  // was: t - cam.step_m
   ```
   `t - cam.step_m` is only correct for fixed-size steps. With adaptive steps, the previous
   bracket is `t_prev` which was saved just before advancing.

Root cause of close-range staircase artifacts: iso-step contours. All rays with the same number
of steps overshoot by the same distance, producing concentric contour lines that look like stairs
on nearby terrain where one step ≈ many pixels. Adaptive steps vary per-ray so contours no longer
align; binary search then refines to sub-step precision.

### Key concepts

- **`COPY_BYTES_PER_ROW_ALIGNMENT = 256`**: wgpu requires `bytes_per_row` to be a multiple of 256
  bytes (= 64 pixels at 4 bytes each). Render buffer width must be rounded up to 64-pixel boundary.
  Actual surface/copy extent uses real pixel width to avoid overrun.
- **render_width vs width**: two separate values maintained in Viewer — `render_width` (aligned, for
  buffer allocation and stride) and `width` (actual, for surface config and copy extent).
- **Sphere tracing for heightmaps**: step = `max(dist_above_terrain * safety, min_step)`.
  Works because the heightmap is a 2.5D surface (no overhangs); the step is always a safe lower
  bound for how far the ray can travel before potentially entering terrain.
- **Binary search bracket**: with variable step sizes, the "previous safe position" must be
  explicitly tracked (`t_prev`). `t - step_m` would give the wrong bracket when step was large.
- **Sky early exit**: with small step_m, sky rays can iterate ~200,000 times before `t > t_max`.
  Early exit at `pos.z > 4000.0 && dir.z > 0.0` cuts this to ~O(1) for sky pixels.

### Open items for this phase
- GPU timestamp queries (measure per-pass GPU time without frame overhead)
- HUD text overlay (`glyphon`)
