# Launcher UI — what we built and why

This document explains every important decision made while building the launcher. The goal is not just to describe what the code does, but to make the *reasons* clear enough that you can apply the same thinking to future problems.

---

## 1. Three egui crates and what each one is responsible for

The egui ecosystem splits its responsibilities across three separate crates:

```
egui          — the UI framework itself: widgets, layout, paint commands, fonts
egui-winit    — translates winit window events (mouse, keyboard, resize) into egui's input format
egui-wgpu     — tessellates egui's paint commands into wgpu vertex buffers and submits them
```

You always need all three. `egui` alone produces a list of paint commands but has no idea how to get input from the OS or how to actually draw anything. `egui-winit` bridges the OS; `egui-wgpu` bridges the GPU.

**The per-frame loop looks like this:**

```
1. egui_winit::State::take_egui_input(window)   → RawInput
2. egui::Context::run(raw_input, |ctx| draw_ui) → FullOutput  (shapes + platform actions)
3. egui_winit::State::handle_platform_output(...)            (cursor changes, clipboard, IME)
4. egui::Context::tessellate(shapes)             → triangles
5. egui_wgpu::Renderer::update_buffers(...)                  (upload vertices to GPU)
6. egui_wgpu::Renderer::render(...)                          (draw call inside a render pass)
7. queue.submit + surface_texture.present()
```

`EguiRenderer` in `renderer.rs` wraps all of this so that the rest of the launcher only calls `egui_renderer.render(window, |ctx| { ... })`.

---

## 2. egui layer ordering — why the loading screen was invisible and the modal was broken

This is the most important concept to understand for the UI bugs we fixed.

egui paints things in **layers**. Every shape, widget, and area belongs to a layer, and layers are drawn in a fixed order defined by `egui::Order`:

```
Background   ← painted first (lowest / behind everything)
Middle
Foreground
Tooltip      ← painted last (highest / in front of everything)
```

Each layer is identified by `egui::LayerId { order: Order, id: Id }`. Within the same `Order` value, layers are sorted by when they were created in that frame.

### Why the loading screen was invisible

The outer panel in `mod.rs` is an `egui::Area` at `Order::Middle`. It draws a semi-transparent dark background via `Frame::NONE.fill(PANEL_BG)`.

The original `loading::show()` created its *own* `egui::Area` at `Order::Background` — below `Middle`. So the order of paint operations was:

```
1. Background: loading panel rectangle (dark grey box)
2. Background: loading text and progress bar
3. Middle: outer panel background fill          ← painted on top, covers everything below
4. Middle: outer panel border
```

The loading content was completely hidden under the panel. We fixed this by removing the separate Area entirely — `loading::show` now just calls `ui.label()`, `ui.allocate_exact_size()` etc. on the `ui` that is passed in. That `ui` is already *inside* the outer panel's Frame, so it renders at `Order::Middle` and is visible.

**Key lesson:** When a function receives a `&mut Ui`, it inherits that Ui's layer. Creating a new layer inside that function at a lower `Order` puts things *behind* the caller's context.

### Why the modal content was behind the background

The original modal code used a `layer_painter` at `Order::Foreground` to draw the background rectangle, and a separate `egui::Area` also at `Order::Foreground` for the content. Both were at the same `Order` level, and the background painter happened to win the z-sort — painting the dark grey box on top of the buttons and text.

The fix: keep the scrim (full-screen dark overlay) at `Order::Foreground`, but move the content Area to `Order::Tooltip`. Since `Tooltip > Foreground`, the content is guaranteed to always be on top.

We also replaced the fixed `modal_h = 360` background rectangle (painted with a separate painter) with `egui::Frame::NONE.fill(...).stroke(...)` applied directly to the Area's content. This way the background auto-sizes to match whatever content height egui computes — no more content overflowing outside the box.

```rust
// OLD: separate painter at Foreground draws fixed-height box; Area at same Foreground may lose z-sort
let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("modal_layer")));
painter.rect_filled(Rect::from_center_size(center, vec2(460.0, 360.0)), ...);
egui::Area::new(id).order(Order::Foreground).show(...);  // may be below the painter

// NEW: scrim at Foreground, content at Tooltip — guaranteed on top; Frame auto-sizes
let scrim = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("modal_scrim")));
scrim.rect_filled(screen, ...);
egui::Area::new(id).order(Order::Tooltip).show(ctx, |ui| {
    egui::Frame::NONE.fill(MODAL_BG).stroke(BORDER).show(ui, |ui| { ... });
});
```

---

## 3. The seamless window transition — why `el.exit()` causes a flash

### What happens on macOS when you call `el.exit()`

`winit` calls `el.exit()` to stop the event loop. On macOS, the event loop teardown includes a call into AppKit that hides (or closes) the associated window. This happens regardless of whether you have a surface, whether the window is still `Arc`-referenced elsewhere, or whether you intend to reuse it. The platform layer (a `CAMetalLayer`) gets destroyed.

When you then create a new window and surface for the viewer, macOS creates a new `CAMetalLayer`, which briefly shows a black frame before the first wgpu render completes. That is the flash.

### The fix: one event loop, two phases

Instead of two separate event loops (one for launcher, one for viewer), we use a single `EventLoop` with a combined `App` handler that holds an enum:

```rust
enum Phase {
    Launcher(LauncherApp),
    Viewer(Viewer),
}
```

All four `ApplicationHandler` methods (`resumed`, `window_event`, `device_event`, `about_to_wait`) delegate to whatever phase is active. After every launcher `window_event`, `try_transition()` checks whether the launcher has set an `outcome`. If it's `LauncherOutcome::Start`, we build a `Viewer` and swap `self.phase = Phase::Viewer(viewer)` **without ever calling `el.exit()`**. The event loop keeps running, the window stays visible, and the next `RedrawRequested` arrives at the viewer instead of the launcher.

```
EventLoop::run_app()
│
├─ App { phase: Phase::Launcher }
│       window_event → launcher handles it
│       try_transition → outcome is None → nothing
│
│   ...user clicks Start, loading completes, launcher sets outcome...
│
├─ App { phase: Phase::Launcher }
│       window_event → launcher handles it
│       try_transition → outcome is Some(Start) → swap to Phase::Viewer
│
└─ App { phase: Phase::Viewer }
        window_event → viewer handles it    ← same OS window, no flash
```

### Why `finish_start()` must not call `el.exit()`

The old launcher was standalone: it called `el.exit()` when done and returned a result. The combined handler makes that pattern illegal — if `el.exit()` is called during the transition, the window hides before the viewer can take over.

`finish_start()` only sets `self.outcome`. The combined handler reads it on the *next* `window_event` call and does the swap. The comment in `mod.rs` captures this:

```rust
// Do NOT call el.exit() — the combined App handler in main.rs observes outcome
// and switches Phase without exiting the event loop, so winit never hides the window.
```

---

## 4. wgpu surface handoff — why we extract it before dropping EguiRenderer

A `wgpu::Surface` is tied to the platform window layer (`CAMetalLayer`, `VkSurfaceKHR`, etc.). As long as the surface object exists, the platform layer stays alive.

`EguiRenderer` owns the surface. When we swap to `Phase::Viewer`, we need the surface to go with the viewer. If we just dropped `EguiRenderer` first, the platform layer would be destroyed and the viewer would need to create a new one — which on macOS causes a brief black frame, exactly the flash we want to avoid.

So `take_surface()` consumes `EguiRenderer` and returns the surface before anything is dropped:

```rust
pub fn take_surface(self) -> wgpu::Surface<'static> {
    self.surface
}
```

The viewer stores the surface in `pre_surface: Option<wgpu::Surface<'static>>`. In `Viewer::resumed()`:

```rust
let surface = if let Some(s) = self.pre_surface.take() {
    s  // reuse — no platform layer recreate
} else {
    scene.get_gpu_ctx().instance.create_surface(window.clone())?  // direct launch
};
```

`SurfaceConfiguration` is not carried across — the viewer recomputes a fresh config from the actual window size in `resumed()`, so passing the launcher's config would be redundant.

---

## 5. `GpuContext::clone()` — what it actually does

`GpuContext` derives `Clone`. This looks like it copies the GPU device, but it doesn't.

Inside wgpu, `wgpu::Device`, `wgpu::Queue`, `wgpu::Adapter`, and `wgpu::Instance` are all thin wrappers around `Arc<...>`. Calling `.clone()` on any of them increments a reference count — you get a new handle pointing at the same underlying GPU object. There is no second device, no second command queue.

This is what makes the single-GPU-context architecture safe: the background loading thread gets cloned handles and builds a `GpuScene` on the same underlying device as the launcher surface. When that `GpuScene` is handed to the viewer, everything is already on the same device, so there are no resource compatibility issues.

```rust
// In resumed() — store the context so we can clone it for the loading thread
self.gpu_ctx = Some(gpu_ctx);

// In begin_loading() — the clone is cheap: just Arc reference count increments
let ctx = self.gpu_ctx.as_ref().unwrap().clone();
std::thread::spawn(move || {
    let prepared = prepare_scene_with_ctx(ctx, ...);  // uses same underlying device
});
```

If we instead called `GpuContext::new()` inside the loading thread (the old approach), we would get a *different* device. The scene's textures and buffers would live on that device, incompatible with the launcher's surface. The viewer would need to re-upload everything.

---

## 6. Progress callbacks — how the loading bar gets updated

The terrain preparation pipeline has five expensive steps: reading the DEM tile, computing normals, computing shadows, computing AO, and uploading to the GPU. Each takes several seconds.

We want the launcher to show live progress during all of these. The natural tool is `mpsc::channel`. But we don't want `prepare_scene_with_ctx` to know anything about channels — that would couple a low-level function to the launcher's architecture.

Instead, the function accepts a generic `report: impl Fn(f32, &str)` callback:

```rust
pub(crate) fn prepare_scene_with_ctx(
    gpu_ctx: GpuContext,
    tile_path: &Path,
    ...
    report: impl Fn(f32, &str),   // (fraction 0.0–1.0, status label)
) -> PreparedScene {
    report(0.05, "Reading terrain data…");
    let hm = load_dem(...);

    report(0.30, "Computing surface normals…");
    let normals = compute_normals(...);
    ...
}
```

The caller provides the closure and decides what to do with the updates:

```rust
// In begin_loading() — closure captures the tx end of the channel
prepare_scene_with_ctx(ctx, ..., |frac, label| {
    let _ = tx.send(LoadProgress { frac, label: label.to_string(), prepared: None });
});
```

The `let _` discards the `Result` because a send failure just means the launcher window was closed — there is nothing to do in that case.

The receiver is polled in `poll_loading()` which is called from the `RedrawRequested` handler every frame:

```rust
fn poll_loading(&mut self, el: &ActiveEventLoop) {
    let Some(rx) = &self.load_rx else { return };
    let mut latest = None;
    while let Ok(msg) = rx.try_recv() {  // drain all waiting messages
        latest = Some(msg);              // keep only the newest
    }
    ...
}
```

`try_recv` is non-blocking. Draining all messages and keeping only the latest avoids the progress bar appearing to "lag behind" when multiple messages pile up between frames.

---

## 7. The resize problem — why the viewer opened at 1600×533 after full-screening

`prepare_scene_with_ctx` receives `WINDOW_W = 1600` and `WINDOW_H = 533` from the module-level constants in `mod.rs`. Those dimensions are embedded in the `GpuScene` (texture sizes, render buffer sizes) and carried in `PreparedScene { width, height }`. `Viewer::from_launcher` copies them into `self.width` and `self.height`.

If the user resizes the window before clicking Start, `self.width / self.height` no longer match the actual window size. When `resumed()` configures the surface with `wgpu::SurfaceConfiguration { width: self.width, height: self.height }`, it uses the stale 1600×533 values. The surface is the wrong size. The viewer will render, but only into a 1600×533 viewport inside a potentially much larger window.

We fix this at the very start of `resumed()`, before any GPU allocation:

```rust
let sz = window.inner_size();
let actual_w = sz.width.max(1);   // .max(1): wgpu panics on zero-size surface
let actual_h = sz.height.max(1);
if actual_w != self.width || actual_h != self.height {
    self.width        = actual_w;
    self.render_width = (actual_w + 63) & !63;  // round up to 64-pixel boundary
    self.height       = actual_h;
    self.scene.as_mut().unwrap().resize(self.render_width, actual_h);
}
```

This reads the *actual* window size from the OS and updates the scene and stored dimensions before any surface configuration happens. All subsequent allocations (surface config, HUD) use the corrected values.

---

## 8. HudBackground vertex buffer bootstrap

`HudBackground::new()` creates a vertex buffer with:

```rust
device.create_buffer(&wgpu::BufferDescriptor {
    size: 144,
    mapped_at_creation: false,  // buffer starts with zero / undefined data
    ...
});
```

The buffer contains 18 vertices (6 triangles × 3 vertices each, 2 floats per vertex = 12 floats per rect × 3 rects = 144 bytes). The actual pixel coordinates depend on the window size — they are written by `update_size(queue, width, height)`.

Before this fix, `update_size` was only called from `Viewer::window_event(WindowEvent::Resized)`. With the combined handler, the viewer never receives a `Resized` event at startup (the window was not resized; the phase just switched). So the buffer stayed all-zeros, and the HUD background rectangles were invisible — three quads of size (0, 0) at the origin.

The fix is to call `update_size` immediately in `HudRenderer::new()`, using the `width` and `height` passed to the constructor:

```rust
let hud_bg = HudBackground::new(device, format);
// Vertex buffer is created without data; write it now before first draw.
hud_bg.update_size(queue, width, height);
```

The constructor now has an explicit `queue` parameter. The comment explains the non-obvious coupling: the buffer must be populated here because no `Resized` event is guaranteed to fire.

---

## 9. egui widgets — `Painter` vs `Ui` and when to use each

egui offers two paths for drawing:

**`Ui` methods** (`ui.label()`, `ui.button()`, `ui.horizontal()`) — participate in the layout flow. Each widget consumes horizontal/vertical space, and the next widget is placed after it. Clicks and hovers are automatically handled.

**`Painter` API** (`painter.text()`, `painter.rect_filled()`, `painter.line_segment()`) — places shapes at absolute pixel coordinates, completely outside the layout flow. No automatic hit testing.

We use `Painter` for everything that is decorative and precisely positioned: the background image, the gradient overlay, the corner marks, row hairlines, the animated hover arrow in `menu_row`. We use `Ui` for anything that needs automatic sizing, word-wrapping, or interactive response: the buttons in the modal, the segmented controls, the breadcrumb link.

`menu_row` is a good example of combining both:

```rust
pub fn menu_row(...) -> bool {
    // Allocate a fixed rect in the layout and get both a Response and a Painter.
    let (response, painter) = ui.allocate_painter(vec2(ui.available_width(), 44.0), Sense::click());
    let rect = response.rect;

    // Everything drawn via painter — absolute positions within rect.
    painter.line_segment([rect.left_top(), rect.right_top()], ...);  // hairline
    painter.text(pos2(rect.min.x + pad, ...), num, ...);             // number column
    painter.text(pos2(rect.min.x + pad + 36.0, ...), label, ...);    // label

    response.clicked()  // return the click result from the Response
}
```

`ui.allocate_painter(size, Sense::click())` does three things at once: reserves `size` pixels in the layout flow, returns a `Response` for input detection, and returns a `Painter` scoped to those exact pixels. This is the standard pattern for fully custom widgets in egui.

---

## 10. Settings index mapping — why all three controls had wrong values

The viewer was written first. Its `vat_mode`, `lod_mode`, and `ao_mode` fields use indices starting from 0 for the *highest quality / most expensive* option:

```
vat_mode:  0=Ultra  1=High  2=Mid  3=Low
lod_mode:  0=Ultra  1=High  2=Mid  3=Low
ao_mode:   0=Off  1=SSAO×8  2=SSAO×16  3=HBAO×4  4=HBAO×8  5=True Hemi
```

When the settings screen was written, the labels were ordered most intuitively for reading — `["Low", "Mid", "High", "Ultra"]` — which put "Low" at index 0. A user selecting "Low" would send `vat_mode = 0`, which the viewer interprets as Ultra. Every option was one or more steps off.

The fix is to match the UI label order to the viewer's index convention:
- `vat_mode` / `lod_mode`: `["Ultra", "High", "Mid", "Low"]`
- `ao_mode`: `["Off", "SSAO×8", "SSAO×16", "HBAO×4", "HBAO×8", "True Hemi."]` — starting from Off (0) so the 6 UI indices 0–5 map directly to shader `ao_mode` 0–5

This kind of mismatch is easy to introduce when the two ends of a data flow are written independently. The safest guard is to write a comment on the type definition that spells out the mapping and confirms which end is authoritative:

```rust
pub vat_mode: u32,   // 0=Ultra 1=High 2=Mid 3=Low   (matches viewer's vat_step_divisors index)
pub ao_mode: u32,    // 0=Off 1=SSAO×8 2=SSAO×16 3=HBAO×4 4=HBAO×8 5=True Hemi (matches shader)
```

---

## 11. `selectable_labels = false` — why egui labels highlight blue on hover

By default, every `egui::Label` is selectable — dragging across it copies text to the clipboard. egui implements this by checking if the mouse is pressed and moving over the label's rect, and if so, applying a selection highlight (the same blue used in text editors).

In a game-style UI where most text is decorative, this is undesirable. The fix is a single global flag set once at startup:

```rust
style.interaction.selectable_labels = false;
```

This disables hit-testing for selection on all labels, so dragging the mouse over "DEM Renderer" or any heading produces no visible response.

---

## 12. Serde defaults — why both `impl Default` and `#[serde(default = "fn")]` exist, and how to avoid duplication

`LauncherSettings` uses serde to persist settings to a TOML file. Two separate mechanisms produce default values:

**`impl Default`** — called from Rust code directly. `toml::from_str(&text).unwrap_or_default()` calls it when the config file is corrupt or missing entirely.

**`#[serde(default = "fn")]` on each field** — called by serde during *partial* deserialization: when the TOML file exists but is missing an individual field (e.g. a user's old config predates the `ao_mode` key). Serde calls the named function to fill in just that one field. Without these attributes, a missing key in the TOML is a deserialization error.

The duplication problem: the same value (`vat_mode` default = 1, `ao_mode` default = 3, etc.) had to be written twice — once in `impl Default` and once in each helper function. If one was changed the other could silently drift.

The fix: make the helper functions delegate to `Default::default()`, so there is exactly one source of truth:

```rust
fn default_vat_mode() -> u32 { LauncherSettings::default().vat_mode }
fn default_ao_mode()  -> u32 { LauncherSettings::default().ao_mode }
```

Now `impl Default` is the only place default values live. Changing `ao_mode: 3` to something else in `Default` automatically updates the serde partial-deserialization default too.

---

## 13. Embedding assets with `include_bytes!`

The background image was originally loaded at runtime:

```rust
image::open("assets/mountain-bg.png")
```

This breaks if the binary is moved to a different directory — the relative path no longer resolves.

`include_bytes!` embeds the raw file bytes directly into the binary at compile time:

```rust
image::load_from_memory(include_bytes!("../../assets/mountain-bg.png"))
```

The macro resolves the path relative to the source file on the build machine — the OS never sees the path string at runtime, so it works from any directory and on any platform (forward slashes work on Windows, Linux, and macOS). The fonts already used this pattern; the background image now does too.

The tradeoff: the binary is larger by the size of the image (~several hundred KB). For a launcher screen this is entirely acceptable.

---

## Summary of architecture decisions

| Decision | Why |
|---|---|
| Single `EventLoop` with `Phase` enum | `el.exit()` hides the window on macOS; one loop eliminates that entirely |
| Surface extracted from `EguiRenderer` via `take_surface()` | Platform layer stays alive during the transition; no black-frame flash |
| `GpuContext::clone()` for loading thread | wgpu handles are Arc-backed; clone shares the same device — scene is compatible with launcher's surface |
| `report: impl Fn(f32, &str)` callback in `prepare_scene_with_ctx` | Decouples the terrain pipeline from the UI channel; function is reusable without a launcher |
| Resize sync at top of `Viewer::resumed()` | No `Resized` event fires on phase switch; actual size must be read from the OS |
| `hud_bg.update_size()` in `HudRenderer::new()` | Vertex buffer is zero at creation; can't rely on a `Resized` event to populate it |
| Modal Area at `Order::Tooltip`, scrim at `Order::Foreground` | Ensures content always renders above the dark overlay regardless of layer insertion order |
| Loading screen renders into caller's `Ui` | Caller is already at `Order::Middle`; creating a sub-layer at `Background` would be invisible |
| Settings labels ordered `["Ultra"…"Low"]` | Matches viewer's 0=Ultra convention; prevents user setting "Low" and getting Ultra |
| Serde helper functions delegate to `Default::default()` | Single source of truth for default values; changing `impl Default` automatically updates partial-deserialization defaults |
| Background image embedded with `include_bytes!` | Binary runs from any directory; no missing-asset crash when moved |
