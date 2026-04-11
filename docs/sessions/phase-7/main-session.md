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
