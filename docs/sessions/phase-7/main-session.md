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
