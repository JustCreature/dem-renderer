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

## Expected Results (GTX 1650, 1600×533)

| Method | fps | bottleneck |
|---|---|---|
| Current `bench_fps` (readback) | ~15 fps | PCIe readback (~47ms) |
| Viewer (surface, no readback) | ~45–50 fps | GPU compute (~20ms) |
| After shader optimisation | TBD | TBD — measure first |

## Open Questions

- Mouse capture: use `window.set_cursor_grab(CursorGrabMode::Locked)` on click,
  release on Escape. Check winit 0.30 API for platform differences.
- Sun direction: fixed for now; later could animate with +/- keys (tie into render_gif sun sweep)
- Window resize: need to reconfigure surface and recreate render_texture on
  `WindowEvent::Resized`
