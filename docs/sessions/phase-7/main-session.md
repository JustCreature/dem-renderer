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

---

## 2026-04-12 (session 6)

### What was covered

**Investigated residual staircase artifact after sphere tracing.**

After sphere tracing + binary search + bilinear normal interpolation, a "ripped stair" look
remained: staircase pattern starts on a slope then is cut off mid-way. Increasing binary
search iterations (8→16) and lowering the safety factor (0.5→0.3) had minimal effect.

**Root cause diagnosed: C1 discontinuity in the bilinear height field.**

The terrain height is bilinearly interpolated (C0 — value continuous) but NOT C1 — the slope
(first derivative) changes abruptly at every DEM grid line (~20m spacing). This creates
visible crease lines in the 3D geometry, independent of normals or shading.

Normal smoothing addresses the shading layer; the geometry crease is visible regardless.

**Attempted fix 1: GPU normal smoothing pass.**
Implemented `shader_smooth_normals.wgsl` — 5×5 separable Gaussian on nx/ny/nz storage
buffers, renormalize after blend. Added smooth pass + 3 smooth buffer fields to
`GpuScene::new()`; render BG pointed at smooth buffers. **Reverted by user** — terrain
became too soft, unacceptable detail loss.

**Attempted fix 2: CPU heightmap smoothing.**
Applied same 5×5 Gaussian to `hm.data` (as `Vec<f32>`) before GPU texture upload; shadow
pass used original data unchanged. **Reverted by user** — same reason.

**Decision: accept DEM resolution as the hard floor.**
~20m/cell SRTM data has inherent C1 discontinuities. `shader_smooth_normals.wgsl` deleted.
No smoothing applied anywhere in the pipeline.

### Key concepts

- **C0 vs C1 continuity**: bilinear interpolation is C0 (height continuous) but not C1
  (slope jumps at every DEM grid boundary). These slope jumps are geometry creases that no
  normal or shading fix can remove.
- **The inescapable trade-off**: any kernel wide enough to eliminate grid-line creases also
  softens real ridgelines. For 20m SRTM data there is no free lunch. The correct fix is
  bicubic height interpolation or higher-resolution source data.

### Open items for this phase
- GPU timestamp queries (measure per-pass GPU time without frame overhead)
- HUD text overlay (`glyphon`)

---

## 2026-04-12 (session 7)

### What was built

**HUD text overlay using glyphon.**

Refactored `src/viewer.rs` → `src/viewer/mod.rs` + `src/viewer/hud_background.rs`.

**Dependency work:**
- Added `glyphon = "0.10.0"` to root `Cargo.toml`
- Discovered version conflict: glyphon 0.10.0 requires wgpu 28, project uses wgpu 29
- Attempted git dependency (github.com/grovesNL/glyphon main) — blocked by Zscaler corporate proxy (`github-zse` DNS failure)
- Attempted `net.git-fetch-with-cli = true` in `~/.cargo/config.toml` — resolved the proxy issue
- Documented fix in `README.md`
- Switched to glyphon git dependency pointing to main branch (wgpu 29 compatible)

**Surface usage fix:**
- Changed surface `TextureUsages` from `COPY_DST` to `COPY_DST | RENDER_ATTACHMENT`
- `RENDER_ATTACHMENT` required for a render pass to target the surface texture

**glyphon integration (4 new fields on `Viewer`):**
- `font_system: glyphon::FontSystem` — CPU font loading + text shaping
- `swash_cache: glyphon::SwashCache` — glyph rasterisation cache
- `text_atlas: glyphon::TextAtlas` — GPU glyph atlas texture (needs `device`, `queue`, `Cache`, `format`)
- `text_renderer: glyphon::TextRenderer` — issues draw calls
- `cache: glyphon::Cache` — GPU-side glyph cache (separate from SwashCache)
- `viewport: glyphon::Viewport` — wraps resolution, replaces old `Resolution` arg in prepare
- `fps_buffer: glyphon::Buffer` — shaped text for fps display, updated every frame
- `hint_buffer: glyphon::Buffer` — static "Q — immersive mode" hint, centered, bottom of screen

**Per-frame pipeline (inserted between copy and submit):**
1. `fps_buffer.set_text(...)` — update fps string
2. `viewport.update(queue, Resolution { width, height })` — sync viewport to surface size
3. `text_renderer.prepare(...)` — upload glyph data, two `TextArea`s (fps top-left, hint bottom-center)
4. `encoder.begin_render_pass(LoadOp::Load)` — preserves terrain, draws text on top
5. `text_renderer.render(...)` — issues draw calls into render pass
6. `drop(rpass)` — releases encoder borrow before `encoder.finish()`

**`HudBackground` struct (in progress):**
- Fields: `pipeline`, `vertex_buf` (96 bytes, 12 vertices), `uniform_buf` (8 bytes, width+height), `bind_group`
- `shader_hud_bg.wgsl`: pixel-space vertex shader (NDC conversion via uniform), fragment outputs `vec4(0,0,0,0.6)`
- Render pipeline with `SrcAlpha / OneMinusSrcAlpha` blend state
- `build_vertices(width, height) -> [f32; 24]`: 2 quads (fps box, hint box), each 2 triangles = 6 vertices

### Key concepts

- **`COPY_DST | RENDER_ATTACHMENT`**: surface texture needs both flags — copy for terrain blit, render attachment for HUD render pass
- **`LoadOp::Load` vs `LoadOp::Clear`**: `Load` preserves terrain; `Clear` would wipe it
- **`drop(rpass)` before `encoder.finish()`**: render pass borrows encoder mutably; must drop to release borrow
- **glyphon `Cache` vs `SwashCache`**: `Cache` is GPU-side atlas management (passed to `TextAtlas::new`); `SwashCache` is CPU-side rasterisation (passed to `prepare`)
- **`Viewport`**: introduced in glyphon 0.10.0, replaces inline `Resolution` — persistent object, updated each frame via `viewport.update(queue, resolution)`
- **Why triangles**: only primitive the GPU rasteriser understands — 3 points always coplanar, unambiguous linear interpolation. Rectangles = 2 triangles sharing a diagonal. Circles = N pizza-slice triangles approximating the curve
- **Fragment shader can render any shape**: SDF/raymarching bypasses triangle limitation by computing "inside/outside" mathematically per pixel

### Open items for this phase
- `HudBackground::update_size` and `draw` methods not yet written
- Wire `HudBackground` into `Viewer` and call in `RedrawRequested`
- GPU timestamp queries

---

## 2026-04-12 (session 8)

### What was built

**`HudBackground` completed** (inside `src/viewer/hud_renderer.rs`):
- `update_size(&self, queue, width, height)`: calls `build_vertices(width, height)`, writes vertex data via `bytemuck::cast_slice`, writes `[width as f32, height as f32]` to uniform buffer
- `draw<'a>(&'a self, rpass: &mut RenderPass<'a>)`: `set_pipeline`, `set_bind_group(0, ...)`, `set_vertex_buffer(0, vertex_buf.slice(..))`, `draw(0..12, 0..1)`

**Viewer module refactored** — `src/viewer/hud_background.rs` renamed to `src/viewer/hud_renderer.rs`:
- `HudBackground` struct remains (private, nested inside the file)
- New `pub struct HudRenderer` wraps all HUD state: `font_system`, `swash_cache`, `text_atlas`, `text_renderer`, `fps_buffer`, `hint_buffer`, `viewport`, `hud_bg: HudBackground`, `width: u32`, `height: u32`
- `HudRenderer::new(device, queue, width, height, format)` — initializes all glyphon state + `HudBackground`
- `HudRenderer::update_size(&mut self, queue, width, height)` — updates stored dimensions, `hint_buffer.set_size`, `hud_bg.update_size`
- `HudRenderer::draw(&mut self, queue, device, encoder, surface_view, fps, ms)` — full render pass: text prepare, `begin_render_pass(LoadOp::Load)`, `hud_bg.draw` (background quads first), `text_renderer.render`, `drop(rpass)`

**`mod.rs` updated:**
- `hud_renderer: Option<HudRenderer>` field (initialized to `None`, set in `resumed()` where `format` is known)
- `HudRenderer::new(...)` called in `resumed()` after format selection — avoids hardcoded format
- `as_mut().expect(...)` for mutable draw/update_size calls

**HUD toggle (E key):**
- `hud_visible: bool` field on `Viewer`, initialized `true`
- `KeyCode::KeyE` in keyboard handler flips `self.hud_visible`
- `RedrawRequested` wraps HUD draw in `if self.hud_visible { ... }`

**Speed boost modifier:**
- `speed_boost: bool` field on `Viewer`
- `KeyCode::SuperLeft/SuperRight` (Mac Cmd) and `KeyCode::AltLeft/AltRight` (Windows Alt) set/clear `speed_boost` on press/release
- Movement block: `let speed = if self.speed_boost { 5000.0 } else { 500.0 }`

### Key concepts

- **`RenderPass<'a>` lifetime**: `draw<'a>(&'a self, rpass: &mut RenderPass<'a>)` — the `'a` unifies the lifetime of `self` (which owns pipeline/bind_group) and the render pass that references them. Without it the compiler cannot prove the borrowed GPU resources outlive the pass recording.
- **`as_ref()` vs `as_mut()`**: `Option::as_ref()` gives `Option<&T>` (shared), `as_mut()` gives `Option<&mut T>` (mutable). Calling a `&mut self` method through `as_ref()` fails with "cannot borrow as mutable".
- **`..` in `slice(..)`**: `RangeFull` — borrows the entire buffer; equivalent to `0..buffer.size()`. The `Buffer::slice` method accepts any range type.
- **`bytemuck::cast_slice`**: reinterprets `&[f32]` as `&[u8]` with a compile-time `Pod` safety check. Both `f32` and `u8` are `Pod` (plain old data — no padding, no pointers).
- **Buffer offset for `write_buffer`**: offset is within the target buffer, not a global address. `vertex_buf` and `uniform_buf` are separate allocations; both are written at offset `0`.
- **`HudRenderer` initialized in `resumed()`** not in `run()`: surface format is only known after surface creation; pipeline format must match or GPU validation fails. `Option<HudRenderer>` handles the delayed init.

### Open items for this phase
- GPU timestamp queries (measure per-pass GPU time without frame overhead)

---

## 2026-04-12 (session 9)

### What was built

**Moving sun animation with background shadow recomputation and soft shadows.**

#### Sun animation (+/- keys)

`Viewer` struct additions:
- `sun_azimuth: f32` — current sun azimuth in radians, wraps `0..2π` via `rem_euclid`
- `sun_elevation: f32` — fixed elevation angle (from initial `SUN_DIR` constant)
- `shadow_tx: mpsc::SyncSender<(f32, f32)>` — sends (azimuth, elevation) to worker
- `shadow_rx: mpsc::Receiver<ShadowMask>` — receives computed masks from worker
- `shadow_computing: bool` — gate: prevents sending new work while worker is busy

Angular velocity:
- Normal: π/30 rad/s (180°/30 s)
- Speed boost active: π/3 rad/s (10×, reuses existing `speed_boost` flag)
- Keys: `KeyCode::Equal` (+) advances, `KeyCode::Minus` retreats

`sun_dir` vector derived from azimuth/elevation each frame (zero-cost, no GPU upload):
```rust
let r = self.sun_elevation.cos();
let sun_dir = [r * self.sun_azimuth.sin(), -r * self.sun_azimuth.cos(), self.sun_elevation.sin()];
```

#### Background shadow worker thread

`prepare_scene` now returns `(GpuScene, Arc<Heightmap>, sun_azimuth, sun_elevation)`.
`Arc<Heightmap>` keeps the CPU heightmap alive after `GpuScene::new()` (which only borrows it
to upload to GPU, then drops it from `prepare_scene`'s stack).

Worker thread spawned once in `run()`:
```rust
let (shadow_tx, worker_rx) = mpsc::sync_channel::<(f32, f32)>(1);
let (worker_tx, shadow_rx) = mpsc::channel::<ShadowMask>();
std::thread::spawn(move || {
    while let Ok((azimuth, elevation)) = worker_rx.recv() {
        let mask = terrain::compute_shadow_vector_par_with_azimuth(...);
        if worker_tx.send(mask).is_err() { break; }
    }
});
```

Per-frame logic (in `RedrawRequested`):
1. `try_recv()` — if shadow ready: `scene.update_shadow(&new_mask)`, `shadow_computing = false`
2. If `!shadow_computing`: `shadow_tx.try_send((azimuth, elevation))` → `shadow_computing = true`

Design rationale:
- `sync_channel(1)` capacity: one pending job max; `try_send` drops stale angles silently
- `shadow_computing` flag prevents queuing more than one job at a time
- Worker finishes in ~1.5ms; frame period ≥ 2.1ms → shadow always ready next frame
- On Win discrete GPU: 52 MB upload via `write_buffer` at ~15fps = ~780 MB/s, well below PCIe 3.0 x16 ceiling

Channel lifecycle: worker's `recv()` returns `Err` when `shadow_tx` is dropped (Viewer destroyed)
→ worker loop exits → thread terminates automatically.

#### Soft shadows (penumbra_meters parameter)

Added `penumbra_meters: f32` to `compute_shadow_vector_par_with_azimuth` and all underlying
implementations (scalar, NEON parallel azimuth, AVX2 parallel azimuth).

Core formula — replaces hard `data[i] = 0.0`:
```rust
let margin = running_max - h_eff;
data[i] = (1.0 - margin / penumbra_meters).max(0.0);
```

NEON version (replaces `vcltq_f32` + `vbslq_f32`):
```rust
let inv_penumbra = 1.0 / penumbra_meters;
let margin = vmaxq_f32(vsubq_f32(running_max, h_eff), vdupq_n_f32(0.0));
let result = vmaxq_f32(
    vsubq_f32(vdupq_n_f32(1.0), vmulq_n_f32(margin, inv_penumbra)),
    vdupq_n_f32(0.0),
);
```

AVX2 version: `_mm256_sub_ps` + `_mm256_mul_ps` + two `_mm256_max_ps` — 8-wide equivalent.
Default `penumbra_meters = 200.0` at all call sites.

Root cause of shadow jerking at slow sun speed: hard 0.0/1.0 mask. At 6°/s individual pixels
on the shadow boundary flipped one at a time → visible jerk. Soft shadows eliminate aliasing
by blending the transition zone.

### Key concepts

- **`Arc<T>` for shared immutable data across threads**: `Arc::clone` increments a reference count
  (cheap); both threads can call `&T` methods concurrently without locks. Needed because `Heightmap`
  never changes after loading.
- **`mpsc::sync_channel(N)` vs `channel()`**: `sync_channel` is bounded — `try_send()` returns
  `Err(Full)` immediately when at capacity. Capacity 1 = at most one pending job, no stale work queues.
- **Channel closure**: dropping the `Sender` causes `recv()` to return `Err` on the receiver.
  Background threads can use this for clean self-termination — no explicit kill signal needed.
- **Shadow margin in metres**: `running_max - h_eff` is the effective-height gap between a pixel
  and the shadow horizon. Normalising by a threshold (metres) gives a 0..1 penumbra blend factor.
- **Rasterisation vs raymarching**: games use shadow maps (render scene from sun POV → depth compare).
  Works for triangle meshes; not applicable to raw heightmaps. DDA horizon sweep is the
  heightmap-equivalent. Shadow maps are embarrassingly parallel → GPU wins. DDA running-max is
  serial per scan line → CPU NEON wins 17×.

### Open items for this phase
- GPU timestamp queries (explicitly deferred — low priority)

---

## 2026-04-13 (session 11)

### What was built

**HUD polish: angular reference fix, date labels, WGSL comments, panel background, drop shadows.**

#### Angular reference corrected

Season circle: Summer (day 172) = top, Winter = bottom. Angle formula already correct from session 10.

Time circle: 12:00 = top, 18:00 = bottom, using 12-hour face. Formula:
```rust
hour_angle = (sim_hour % 12.0) / 12.0 * TAU
```
Cardinal label text updated accordingly: "12:00" at top, "18:00" at bottom, "15:00" right, "21:00" left.

#### Current-value labels: date format

Replaced "Day 156" with a proper `"Jun 21"` style date via `day_to_date(day: i32) -> String`:
```rust
fn day_to_date(day: i32) -> String {
    const MONTHS: [(&str, i32); 12] = [ ("Jan", 31), ("Feb", 28), ... ("Dec", 31) ];
    let mut rem = day.clamp(1, 365);
    for (name, days) in &MONTHS {
        if rem <= *days { return format!("{} {}", name, rem); }
        rem -= days;
    }
    "Dec 31".to_string()  // satisfies compiler; unreachable
}
```
The final fallback is mathematically unreachable (input is clamped to 1–365 and total days = 365) but required because Rust cannot prove the loop always returns.

Time label changed to `format!("Time: {:02}:{:02}", h, m)` using `sim_hour.fract() * 60.0` for minutes.

Removed unused `season_name` function.

#### `shader_sun_hud.wgsl` — detailed comments added

Every function and code section annotated explaining:
- `TAU = 2π` convention
- `seg_sdf` clamped-projection math
- `frag_angle` clockwise-from-top via `atan2(dp.x, -dp.y)` + Y-flip
- `season_col` boundary angles in radians
- `tick` dir = `vec2(sin, -cos)` clock-angle convention
- `draw_circle` layer ordering (disc → ring → ticks → needle → centre dot)
- `fs_main` discard guard rationale ("~99% of full-screen quad pixels discarded at near-zero cost")

#### HUD background panel (`panel_rect_sdf` in WGSL)

Replaced two small per-label rects with one unified rounded panel covering both circles and all labels:

```wgsl
fn panel_rect_sdf(p: vec2<f32>) -> f32 {
    let cx = u.cx1;  // cx1 == cx2
    let r  = u.radius;
    let x0 = cx - r - 112.0;   // past current-value labels (~10 px padding)
    let x1 = cx + r + 72.0;    // past Fall/15:00 labels
    let y0 = u.cy1 - r - 28.0; // above Summer/12:00
    let y1 = u.cy2 + r + 28.0; // below Winter/18:00
    // rounded-rect SDF, corner radius 8 px
    ...
}
```

`fs_main` restructured: discard if outside circles AND outside panel → draw circles first (they have their own disc background) → draw panel dark background (0.05, 0.05, 0.05, 0.60) for pixels in panel but outside circles.

Design rationale: circles already have a semi-transparent disc fill — the panel fills the gaps between circles and around all labels without double-layering inside the circles.

#### Drop shadows on all text labels

Each of the 10 glyphon text buffers (8 cardinal + 2 current-value) is now rendered twice in `text_renderer.prepare()`:
1. **Shadow pass** — same buffer, offset `(+1, +1)` pixels, `Color::rgba(0, 0, 0, 160)`
2. **Real pass** — normal position, `Color::rgb(210, 210, 210)`

glyphon composites `TextArea`s in slice order, so shadow lands under real glyphs automatically. Total `TextArea` count: 2 (fps + hint) + 10 shadow + 10 real = 22.

### Key concepts

- **`day_to_date` unreachable fallback**: Rust requires all code paths to return a value. A mathematically exhaustive loop (all 365 days covered by month sums) still needs a trailing `return` or `unreachable!()` to satisfy the compiler. `"Dec 31".to_string()` is the minimal correct choice.
- **Glyphon double-render for drop shadow**: render the same `glyphon::Buffer` twice — once offset with dark `Color::rgba`, once at correct position with light colour. No separate shadow buffer needed; order in the `TextArea` slice determines draw order.
- **Single panel vs two small rects**: two small per-label rects leave the cardinal labels (Summer/Fall/Winter/Spring etc.) unprotected against busy terrain. One big panel covers the entire widget uniformly. The circles' own disc fill prevents redundant layering inside them.
- **`panel_rect_sdf` in WGSL using existing uniform**: the panel bounds are computable from `cx1`, `cy1`, `cy2`, `radius` already in the `SunHud` uniform — no new GPU data upload needed.

### Open items for this phase
- GPU timestamp queries (explicitly deferred — low priority)

---

## 2026-04-12 (session 10)

### What was built

**Geographically correct sun position (replacing fixed-elevation azimuth rotation).**

Replaced `sun_azimuth`/`sun_elevation` fields with:
- `sim_day: i32` — day of year 1–365
- `sim_hour: f32` — solar time 0.0–24.0
- `lat_rad: f32` — tile centre latitude in radians (derived from `hm.origin_lat` and `hm.dy_deg`)
- `day_accum: f32` — fractional day accumulator for sub-frame `[`/`]` key increments

Free function `sun_position(lat_rad, day, hour) -> (azimuth, elevation)` using Spencer 1971:
```rust
let decl = 23.45_f32.to_radians()
    * ((360.0 / 365.0 * (day as f32 + 284.0)).to_radians()).sin();
let h    = (15.0 * (hour - 12.0)).to_radians();  // solar hour angle
let sin_el = lat_rad.sin()*decl.sin() + lat_rad.cos()*decl.cos()*h.cos();
let elevation = sin_el.clamp(-1.0, 1.0).asin();
let cos_az = (decl.sin() - sin_el*lat_rad.sin()) / (elevation.cos()*lat_rad.cos());
let azimuth = if h > 0.0 { TAU - cos_az.acos() } else { cos_az.acos() };
```
Key controls: `+`/`-` change `sim_hour` at 0.4 hrs/s (4 hrs/s boosted); `[`/`]` change `sim_day` via `day_accum`. Shadow only dispatched when `elevation > 0.0`.

**CPU normals replace GPU-computed normals in `GpuScene::new()`.**

Removed the 157-line throw-away GPU compute pipeline (shader compile + dispatch + poll) and replaced with three `create_buffer_init` DMA uploads from the `NormalMap` computed by `terrain::compute_normals_vector_par`. `GpuScene::new()` signature gains a `normal_map: &NormalMap` parameter; all call sites updated.

**Sun / Season HUD — `SunIndicator` struct + `shader_sun_hud.wgsl`.**

`src/viewer/shader_sun_hud.wgsl` — new SDF circle shader:
- Full-screen NDC vertex pass; all math in fragment shader using `@builtin(position)` pixel coords
- `frag_angle(p, center)` — angle clockwise from top (0 = up, π/2 = right)
- `season_col(angle)` — season-coloured outer ring: yellow/orange/blue/green by angular sector
- `draw_circle(p, center, r, needle, kind)` — semi-transparent disc + coloured ring + 4 tick marks + yellow SDF needle + white centre dot
- `SunHud` uniform (48 bytes): `screen_w/h`, `cx1/cy1` (season), `cx2/cy2` (time), `radius`, `day_angle`, `hour_angle`, `_pad×3`
- `discard` guard: fragments outside both circles cost nearly nothing

`SunIndicator` struct in `hud_renderer.rs`:
- `pipeline`, `vertex_buf` (static NDC quad, 6 vertices), `uniform_buf` (48 bytes, written every frame), `bind_group`
- Cached `cx`, `cy1`, `cy2` so `HudRenderer::draw()` can position text labels
- Constants: `RADIUS=50`, `RIGHT_MARGIN=80` (room for right-side labels), `BOTTOM_OFFSET=118`, `GAP=60`

**Angular reference (Summer=top, 12:00=top).**

Season circle uses a 365-day face with summer solstice at 12 o'clock:
```rust
day_angle = (sim_day as f32 - 172.0).rem_euclid(365.0) / 365.0 * TAU
```
Time circle uses a 12-hour clock face with noon at 12 o'clock:
```rust
hour_angle = (sim_hour % 12.0) / 12.0 * TAU
```

Season tick angles: `0` (Summer), `93/365·τ` (Fall), `183/365·τ` (Winter), `273/365·τ` (Spring).
Time tick angles: `0` (12:00), `τ/4` (15:00), `τ/2` (18:00), `3τ/4` (21:00).

**Text label system — 10 glyphon buffers.**

- 8 static cardinal labels: "Summer" (above), "Winter" (below), "Fall" (right), "Spring" (left) for season circle; "12:00"/"18:00"/"15:00"/"21:00" for time circle
- 2 dynamic current-value labels: "Day 156" right-aligned at 10-11 o'clock outside season ring; "17:44" at 10-11 o'clock outside time ring
- Helper functions: `make_small_label()`, `make_current_label()`, `label_area()` reduce boilerplate

### Key concepts

- **Solar declination (Spencer 1971)**: `δ = 23.45° × sin(360°/365 × (day + 284))` — gives declination in radians for any day of year. Combined with solar hour angle `H = 15° × (hour − 12)` and observer latitude, gives sun elevation and azimuth.
- **Solar hour angle**: `H = 15° × (hour − 12)` — 0 at noon, negative in the morning, positive in the afternoon. Used to flip the azimuth formula for AM vs PM.
- **`create_buffer_init` vs GPU compute for normals**: throwing away a compute pipeline after one use compiles the shader, builds a pipeline object, dispatches, and polls — O(100ms) on first call. `create_buffer_init` is a DMA memcpy: same data, 1000× cheaper.
- **SDF circle in a fragment shader**: place a full-screen NDC quad so every screen pixel runs the fragment shader. Compute `d = length(p - center)`. Everything else — ring, disc, needle, ticks — is analytic distance field math in pixel space. `discard` for `d > r + margin` prunes 99% of pixels to near-zero cost.
- **`frag_angle` clockwise from top**: `atan2(dp.x, -dp.y)` — negating `dp.y` flips the Y axis (pixel space is Y-down), giving clockwise angle with 0 at the top. Add `TAU` if negative to map to `[0, TAU)`.
- **12h clock face for a 24h variable**: `(hour % 12.0) / 12.0 * TAU` maps 12:00 and 24:00 both to 0 (top), 18:00 to `τ/2` (bottom). Noon and midnight share the same visual position, which is correct for a solar-time indicator where only AM/PM matters for the sun.
- **`rem_euclid` for angular wrap**: `(day - 172).rem_euclid(365)` gives days-after-solstice wrapping correctly for days 1–171 (which are after the previous winter). Unlike `%`, `rem_euclid` is always non-negative.
- **`day_accum` for smooth key-driven increments**: accumulate fractional days each frame at `dt × speed`; only advance `sim_day` when `day_accum ≥ 1.0`, keeping the remainder. Without this, `[`/`]` at 60fps would require 60 key-presses per day.

### Open items for this phase
- GPU timestamp queries (explicitly deferred — low priority)

---

## 2026-04-13 (session 12)

### What was covered

**Conceptual planning: viewer improvement directions.**

No code written. Three topics explored and formalised into `docs/planning/viewer-improvements-plan.md`.

#### Out-of-core tile streaming

**Problem:** current single-tile setup fits entirely in GPU VRAM. Multi-tile datasets (e.g. 10×10 SRTM region, ~260 MB) would exceed VRAM on most hardware.

**Solution architecture:**
- Spatial tiling on disk (fixed-size chunks, seekable)
- GPU tile cache — fixed-size 2D texture array, LRU eviction
- Indirection table (page table texture) — shader looks up `tile_id → slot_index`

**Hardware angle:** CPU→GPU upload is the bottleneck. On M4 unified memory, near-free. On PCIe: 26 MB tile ≈ 1.7ms; 4 tiles/frame = 6.8ms stall. Measurable question: at what camera speed does streaming become the fps ceiling? Equivalent GPU API concept: sparse/virtual textures ("tiled resources" in DirectX, "sparse binding" in Vulkan).

#### Level of Detail (LOD)

**Problem:** at 50km distance, one DEM cell = 0.008 screen pixels — full-precision raymarching for sub-pixel terrain is wasted work.

**Three approaches:**
1. Step-size LOD — `min_step = base_step * (1 + t / 5000.0)`, 2-line shader change
2. Mipmap LOD — multiple downsampled heightmap versions; `textureSampleLevel(hm, s, uv, lod)` already supports this; costs 1.33× GPU memory
3. Geometry LOD — not applicable to heightmap raymarching (no triangle mesh)

**Hardware angle:** smaller mip → better GPU L2 cache fit → fewer cache misses per ray. Larger effect on GTX1650 (small GDDR6 cache) than M4 (large unified L2).

#### Ambient Occlusion (AO)

**Physical intuition:** sky hemisphere partially blocked by surrounding geometry → valleys darker than ridges even on overcast days.

**Four switchable modes** cycled with `/` key; HUD label shows `AO: <mode>  (Press / to change)`:

- **Off** — AO factor = 1.0, baseline fps reference
- **SSAO** — N random hemisphere samples per fragment per frame; N texture fetches → measurable fps cost; random access pattern → low cache hit rate
- **HBAO** — D directional marches per fragment, find max horizon elevation angle; more physically accurate than SSAO; diagonal directions hit same cache-unfriendly stride as Phase 3 diagonal shadow (2.4× slower than cardinal)
- **True Hemisphere** — full DDA horizon sweep in N directions baked to R8Unorm texture at startup; render-time cost = one texture fetch; startup ≈ N × 1.5ms ≈ 24ms for N=16; best quality, no view-dependent artifacts

**Relationship to existing code:** sun shadow = single-direction specialisation of True Hemisphere AO. Same `compute_shadow_neon_parallel_with_azimuth`, 16 azimuths, averaged.

### Artifact created

`docs/planning/viewer-improvements-plan.md` — new planning file with three improvement items fully detailed: AO (item 1, four modes, `/` key cycling, HUD label, comparison experiment), out-of-core streaming (item 2), LOD (item 3).

### Open items for this phase
- GPU timestamp queries (explicitly deferred — low priority)
- Implement viewer improvements from `viewer-improvements-plan.md` (AO first)

---

## 2026-04-16 (session 13)

### What was covered

**AO infrastructure (step C) — key binding, scene uniform, settings HUD.**

All wiring done; no shader AO logic yet.

#### Plan update: 6 AO modes instead of 4

Expanded the mode list to give each sample count its own slot — makes the scaling curve directly measurable:

| Mode | ao_mode | Notes |
|---|---|---|
| Off | 0 | Baseline |
| SSAO ×8 | 1 | N=8 random hemisphere samples |
| SSAO ×16 | 2 | N=16; measures whether fps drops 2× or more |
| HBAO ×4 | 3 | D=4 directions |
| HBAO ×8 | 4 | D=8; diagonal stride penalty measurable |
| True Hemi | 5 | Precomputed, render-time = one texture fetch |

`rem_euclid(6)` wraps the cycle. One shader with uniform branch on `ao_mode` — zero wave divergence since all threads share the same mode value.

`ao_mode` lives on the scene uniform (with camera/render parameters), not the sun HUD uniform. Settings HUD is a new separate panel, extensible for future settings.

`viewer-improvements-plan.md` updated to reflect 6 modes, uniform branch rationale, and settings HUD.

#### Code changes

**`crates/render_gpu/src/camera.rs`**
- `ao_mode: u32` added to `CameraUniforms` struct (replaced `_pad5`)
- Passed through `CameraUniforms::new()`

**`crates/render_gpu/src/scene.rs`**
- Both `CameraUniforms::new()` call sites (lines 334, 433) now take `ao_mode`

**`crates/render_gpu/src/shader_texture.wgsl`**
- `ao_mode: u32` added to WGSL `CameraUniforms` struct (same byte position as Rust)
- Not yet read by shader logic — wired but unused until AO modes implemented

**`src/viewer/mod.rs`**
- `ao_mode: u32` field on `Viewer`, initialised to `0`
- `/` key (`KeyCode::Slash`) handler: `self.ao_mode = (self.ao_mode + 1).rem_euclid(6)`
- `ao_mode` passed to both `CameraUniforms::new()` and `hud_renderer.draw()`

**`src/viewer/hud_renderer.rs`**
- `settings_buffer: glyphon::Buffer` field — top-right settings HUD label
- `draw()` signature extended with `ao_mode: u32`
- `match ao_mode` produces label string (`"AO: Off  (Press / to change)"` etc.)
- `settings_buffer.set_text()` called each frame with current label
- `build_vertices` extended to `[f32; 36]` — third rect for settings background (top-right, `x0=w-296, x1=w-4, y0=4, y1=36`)
- GPU buffer size: `96 → 144` bytes; draw call: `0..12 → 0..18`
- TextArea added for `settings_buffer` at `left: w - 292.0, top: 10.0`

#### Key concepts discussed

- **Wave / warp / wavefront**: GPU executes threads in fixed-size lockstep groups (~32 threads on NVIDIA, 64 on AMD). All threads in a wave execute the same instruction each cycle. A uniform branch (same condition for all threads) costs ~zero — the wave takes one path. A divergent branch (different threads take different paths) forces both paths to execute with masks — up to 2× cost.
- **`rem_euclid` vs `%`**: `%` on signed integers can return negative values; `rem_euclid` always returns non-negative. For `u32` the result is identical, but `rem_euclid` signals "wrap in a cycle" intent clearly.
- **Two-triangle rectangle**: each rect = 2 triangles × 3 vertices × 2 floats. T1 = top-left → top-right → bottom-right; T2 = top-left → bottom-right → bottom-left. The diagonal always runs top-left to bottom-right.
- **Buffer width must match TextArea left**: if `settings_buffer` width is 400px but `left = w - 300`, text overflows the screen by 100px. Fix: set buffer width = rect width (292px) and `left = w - 296`.

### Next step
Implement True Hemisphere AO (ao_mode == 5):
- CPU: `compute_ao_true_hemi(hm, n_directions=16, penumbra_meters)` in `crates/terrain/src/shadow.rs` — sweep 16 evenly-spaced azimuths at elevation=0, accumulate lit fractions, average → `Vec<f32>`
- GPU upload: convert to `u8` (× 255), upload as `R8Unorm` texture in `GpuScene::new()`
- Shader: sample AO texture at hit UV, multiply into colour when `ao_mode == 5`

### Open items for this phase
- GPU timestamp queries (explicitly deferred — low priority)
- AO implementation: True Hemi (next), then SSAO ×8/×16, then HBAO ×4/×8
