# Interactive Viewer — Implementation Plan

## Goal

Add a real-time interactive terrain viewer invoked via `./dem_renderer --view`.
The viewer eliminates the CPU readback bottleneck (currently ~47ms/frame on GTX 1650)
by rendering directly into a wgpu Surface (swap chain), so pixels never cross PCIe.

## Motivation

Current `bench_fps` measures GPU compute + 3.4 MB PCIe readback.
On GTX 1650: ~20ms GPU compute + ~47ms readback = 67ms/frame = 15 fps.
The 15 fps number measures the wrong thing — mostly PCIe bandwidth, not GPU throughput.

With a Surface:
- No readback at all → GPU compute ceiling (~20ms) becomes the bottleneck
- Expected: ~50 fps on GTX 1650, limited by shader throughput
- Later: GPU timestamp queries give exact shader time without wall-clock noise

## Design Decisions (already resolved)

- Lives in `src/viewer.rs` as a module, not a separate binary target
- Invoked with `--view` flag: `./dem_renderer --view`
- Without `--view`, all existing benchmarks run unchanged
- Interactive: WASD camera movement + mouse look
- `bench_fps` stays unchanged until the new render path is validated
- Render into intermediate storage texture, blit to surface texture
  (safer than writing directly to surface: not all platforms support
   `STORAGE_BINDING` on surface formats)

## Architecture

```
src/viewer.rs
└── pub fn run(heightmap, normal_map, shadow_mask)
    ├── create winit window + event loop
    ├── create wgpu Surface from window
    ├── Viewer struct
    │   ├── surface: wgpu::Surface
    │   ├── config: wgpu::SurfaceConfiguration
    │   ├── render_texture: wgpu::Texture   ← compute shader writes here
    │   ├── camera: CameraState
    │   ├── gpu_scene: GpuScene             ← reuse existing Phase 5 struct
    │   └── input: InputState
    └── event loop
        ├── WindowEvent::KeyboardInput  → update InputState
        ├── DeviceEvent::MouseMotion    → update camera yaw/pitch
        └── WindowEvent::RedrawRequested
            ├── update camera from InputState (WASD)
            ├── write camera uniforms (128 bytes)
            ├── dispatch compute → render_texture
            ├── copy_texture_to_texture → surface texture
            └── surface_texture.present()
```

## Dependencies to add

```toml
# render_gpu/Cargo.toml  (or root Cargo.toml — viewer is in src/, not a crate)
winit = "0.30"
```

`winit 0.30` changed the event loop API significantly from 0.29 — use `ApplicationHandler`
trait instead of the old closure-based `run()`. Look up `winit::application::ApplicationHandler`.

## Implementation Steps (in order)

### Step 1 — `--view` flag in main.rs
Parse `std::env::args()`. If `--view` present: load heightmap + normals + shadow, call
`viewer::run(...)`, return early (skip all benchmarks).

### Step 2 — Window + Surface setup
```
winit EventLoop::new()
winit WindowBuilder → Window
wgpu Instance::create_surface(&window)
configure surface: format = surface.get_capabilities().formats[0]
                   present_mode = PresentMode::Fifo (vsync) or Mailbox (uncapped)
                   width/height from window inner_size
```

### Step 3 — Intermediate render texture
```
device.create_texture(
    size: (width, height),
    format: TextureFormat::Rgba8Unorm,
    usage: STORAGE_BINDING | COPY_SRC,   ← compute writes here
)
```
The existing shaders write to `array<u32>` storage buffer. Two options:
- **Option A (less work):** keep buffer output, use `copy_buffer_to_texture` each frame
- **Option B (cleaner):** change shader output to `texture_storage_2d<rgba8unorm, write>`

Start with Option A to get something running, switch to Option B later.

### Step 4 — CameraState + InputState
```rust
struct CameraState {
    pos: [f32; 3],
    yaw: f32,    // horizontal angle (radians)
    pitch: f32,  // vertical angle (radians)
    fov_deg: f32,
}

struct InputState {
    forward: bool, back: bool,
    left: bool, right: bool,
    up: bool, down: bool,
    speed: f32,
}
```
Each frame, advance position along `(yaw, pitch)` direction vector by
`speed * dt` for each held key. `dt` = elapsed since last frame (use `std::time::Instant`).

### Step 5 — Per-frame render loop
```
surface_texture = surface.get_current_texture()
update camera_uniforms from CameraState
write camera_uniforms to GpuScene (128 bytes via queue.write_buffer)
dispatch compute → render_texture
encoder.copy_buffer_to_texture(render_texture → surface_texture)   // Option A
surface_texture.present()
window.request_redraw()   // trigger next frame immediately
```

### Step 6 — FPS counter
Track last 60 frame times in a ring buffer. Print average fps to window title every second:
```rust
window.set_title(&format!("dem_renderer  {:.0} fps  {:.1} ms", fps, ms));
```

## Later: GPU Timestamp Queries (bench_fps improvement)

Once the viewer render path exists:
1. Add `TIMESTAMP_QUERY` feature to `wgpu::DeviceDescriptor`
2. `device.create_query_set(QueryType::Timestamp, 2)`
3. `encoder.write_timestamp(&qs, 0)` before dispatch, `write_timestamp(&qs, 1)` after
4. Resolve query to a 16-byte buffer, read back those 16 bytes (not 3.4 MB)
5. Subtract timestamps, multiply by `queue.get_timestamp_period()` → nanoseconds

This gives pure shader execution time, eliminating all PCIe and synchronization noise.
Replace the wall-clock measurement in `bench_fps` with this.

## Actual Results (M4 Max, measured 2026-04-11)

| Method | Resolution | fps | bottleneck |
|---|---|---|---|
| `bench_fps` (readback) | 8000×2667 | ~10 fps | 85 MB readback (~88ms) |
| Viewer, vsync off | 1600×533 | 470 fps | command overhead floor (~2.1ms) |
| Viewer, vsync on | 1600×533 | 100 fps | display refresh rate |
| Viewer, vsync off | 8000×2667 | 21 fps | GPU compute (~47ms) |
| Viewer, step_m tiny (dx/100) | 1600×533 | 17 fps | GPU compute |

Texture cache experiment (fixed step_m=4.0m, factors 1/2/4/8): identical fps at all factors —
bottleneck is compute throughput (loop iterations), not texture fetch bandwidth on M4.

## Current Implementation State (2026-04-11)

All planned steps are complete. Additional features beyond the original plan:

**`src/viewer.rs` — `Viewer` struct fields:**
- `scene: Option<GpuScene>` — GPU resources
- `window`, `surface`, `surface_config` — winit + wgpu surface
- `width`, `height`, `vsync: bool`
- `cam_pos: [f32; 3]`, `yaw: f32`, `pitch: f32` — camera state (replaces planned `CameraState`)
- `keys_held: HashSet<KeyCode>` — held keys (replaces planned `InputState`)
- `mouse_look: bool`, `immersive_mode: bool` — input mode flags
- `last_frame: Instant` — delta time for movement
- `fps_timer: Instant`, `frame_count: u32`, `fps: f64` — FPS display

**Controls:**
- WASD — horizontal movement (speed 500 m/s)
- Space / ShiftLeft — vertical movement
- Left mouse button held — drag to look (normal mode)
- Q — toggle immersive mode (cursor locked, mouse always controls look)
- `--vsync` CLI flag — enables `PresentMode::Fifo`

**`crates/render_gpu/src/scene.rs` additions:**
- `dispatch_frame(&mut encoder, origin, look_at, fov, aspect, sun_dir, step_m, t_max)`
- `get_output_buffer()`, `get_gpu_ctx()`, `get_dx_meters()`, `get_dy_meters()`

**`crates/render_gpu/src/context.rs` additions:**
- `pub instance: wgpu::Instance`, `pub adapter: wgpu::Adapter` (needed to create surface from same instance)

**`shader_texture.wgsl`:** pixel packing changed to BGRA byte order for Metal `Bgra8Unorm` surface.

## Open Items

- **Window resize** — `WindowEvent::Resized` not handled; resizing causes surface/buffer mismatch
- **Sun direction** — fixed; no animation yet
- **GPU timestamp queries** — see section below
- **HUD text** — see section below

## Later: HUD Text Overlay

### Approach
Use the `glyphon` crate — minimal wgpu-native text renderer. Composites glyph atlas over
the rendered frame via a second render pass after the compute blit.

### Required change to surface config
Add `RENDER_ATTACHMENT` to surface usage (currently only `COPY_DST`):
```rust
usage: TextureUsages::COPY_DST | TextureUsages::RENDER_ATTACHMENT,
```

### Objects to store in `Viewer` (set up once in `resumed()`)
```rust
font_system:   glyphon::FontSystem
swash_cache:   glyphon::SwashCache
text_atlas:    glyphon::TextAtlas
text_renderer: glyphon::TextRenderer
```

### Per-frame pattern
```
// after copy_buffer_to_texture, before present():
let view = surface_texture.texture.create_view(&Default::default());
text_renderer.prepare(&device, &queue, &mut font_system, &mut text_atlas,
    Resolution { width, height },
    [TextArea { buffer, left, top, scale, bounds, default_color }],
    &mut swash_cache,
)?;
let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
    color_attachments: [Some(RenderPassColorAttachment {
        view: &view,
        load: LoadOp::Load,   // ← keep existing pixels, draw text on top
        store: StoreOp::Store,
    })],
    ..
});
text_renderer.render(&text_atlas, &mut pass)?;
```

### Font
Embed with `include_bytes!("some-font.ttf")` or use `glyphon`'s system font discovery.

### HUD content planned
- fps + ms/frame (top-left)
- camera position in world meters (top-left)
- current mode indicator: "normal | LMB: look" vs "immersive | Q: exit"
- crosshair (can be a texture or just 4 pixels written in the compute shader)

### Estimated effort
~1–2 hours: ~30–40 lines of glyphon setup + per-frame render pass.
Non-obvious: `glyphon` types are not `Send`; keep all on render thread (already satisfied).

## Later: GPU Timestamp Queries (bench_fps improvement)

Once the viewer render path exists:
1. Add `TIMESTAMP_QUERY` feature to `wgpu::DeviceDescriptor`
2. `device.create_query_set(QueryType::Timestamp, 2)`
3. `encoder.write_timestamp(&qs, 0)` before dispatch, `write_timestamp(&qs, 1)` after
4. Resolve query to a 16-byte buffer, read back those 16 bytes (not 3.4 MB)
5. Subtract timestamps, multiply by `queue.get_timestamp_period()` → nanoseconds

This gives pure shader execution time, eliminating all PCIe and synchronization noise.
Replace the wall-clock measurement in `bench_fps` with this.

## Later: Picture Quality Improvements

### Root cause of "big tile" appearance
`textureLoad` snaps to the nearest integer texel. Each DEM cell is ~21m. At close range
one cell covers many screen pixels → hard edges between cells are visible as blocky patches.

### Priority 1 — Bilinear height interpolation (biggest visual win, shader only)

Replace `textureLoad` with `textureSampleLevel` in the ray loop and binary search.
The sampler is already bound at `@binding(2)` but currently unused.

Current:
```wgsl
let h = textureLoad(hm_tex, vec2<i32>(col, row), 0).r;
```

Replace with (in both the main loop and `binary_search_hit`):
```wgsl
let uv = vec2<f32>(pos.x / (f32(cam.hm_cols) * cam.dx_meters),
                   pos.y / (f32(cam.hm_rows) * cam.dy_meters));
let h = textureSampleLevel(hm_tex, hm_sampler, uv, 0.0).r;
```

`textureSampleLevel` interpolates between the 4 surrounding height values (bilinear),
eliminating the hard cell-to-cell steps. LOD 0 = full resolution, no mip blurring.

Note: `textureSample` is not allowed in compute shaders without uniform control flow;
use `textureSampleLevel` (explicit LOD) which is permitted everywhere.

### Priority 2 — Smooth elevation color transitions (shader only)

Current hard cutoffs:
```wgsl
if pos.z < 1900.0 { green }
else if pos.z < 2100.0 { slightly green }
...
```

Replace with `mix` over a transition zone (±100m):
```wgsl
let t = smoothstep(1800.0, 2000.0, pos.z);  // 0.0 at 1800m, 1.0 at 2000m
let col = mix(green, rock, t);
// chain for each band boundary
```
Eliminates the hard stripe at each elevation threshold.

### Priority 3 — Normal interpolation (more work, moderate gain)

Normals are in SoA storage buffers (`nx`, `ny`, `nz`) accessed by integer index:
```wgsl
let normal = vec3<f32>(nx[idx], ny[idx], nz[idx]);
```

For smooth shading, bilinearly blend 4 neighbours:
- Compute `(col_f, row_f)` fractional position within cell
- Read `nx/ny/nz` at `(col, row)`, `(col+1, row)`, `(col, row+1)`, `(col+1, row+1)`
- `mix(mix(n00, n10, frac_x), mix(n01, n11, frac_x), frac_y)` then re-normalise
- 4× more buffer reads per hit pixel — measure fps impact

### Estimated effort
- Priority 1: ~10 lines changed in `shader_texture.wgsl`, no Rust changes
- Priority 2: ~20 lines changed in `shader_texture.wgsl`, no Rust changes
- Priority 3: ~30 lines changed in `shader_texture.wgsl`, no Rust changes

## Open Questions

- Sun direction: fixed for now; later could animate with +/- keys (tie into render_gif sun sweep)
- Window resize: need to reconfigure surface and recreate output buffer on `WindowEvent::Resized`
