//! Browser PoC entry point for the DEM renderer.
//!
//! Viewer-only: the launcher / streaming tiers / native I/O are bypassed entirely. The
//! user clicks the canvas to pick a single GeoTIFF; its bytes are parsed and normals are
//! computed on a `wasm-bindgen-rayon` worker (SharedArrayBuffer), then the main thread
//! builds a `GpuScene` and drives the existing compute raymarcher every frame.
//!
//! All wgpu handles stay on the main thread (they are `!Send` on wasm-with-atomics); only
//! plain data (`Heightmap` / `NormalMap`) crosses the worker boundary via an mpsc channel.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use wasm_bindgen::prelude::*;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowAttributes, WindowId};

use dem_io::Heightmap;
use render_gpu::{GpuContext, GpuScene};
use terrain::{NormalMap, ShadowMask};

/// Fixed render target. Width must be a multiple of 64 so `bytes_per_row` (width * 4) is
/// 256-byte aligned for `copy_buffer_to_texture`. Kept modest because the raymarch is
/// per-pixel on the GPU and weak/integrated GPUs (e.g. 2019 Intel Macs) otherwise saturate
/// and stall the macOS compositor. Bump for sharper output on a stronger GPU.
const RENDER_W: u32 = 640;
const RENDER_H: u32 = 480;

/// Cap on grid side length. The per-cell GPU buffers (packed normals, f32 shadow) are
/// 4 bytes/cell and must each stay under WebGPU's default 128 MB storage-binding limit:
/// 5120² × 4 ≈ 100 MB (safe), whereas 6144² × 4 ≈ 151 MB would exceed it. Larger tiles are
/// center-cropped to this.
const MAX_TILE_DIM: usize = 5120;

/// Re-export so the page's JS can `await initThreadPool(n)` before any load happens.
pub use wasm_bindgen_rayon::init_thread_pool;

/// CPU-side scene inputs produced off the main thread by the worker pool.
struct Loaded {
    hm: Heightmap,
    normals: NormalMap,
}

#[wasm_bindgen(start)]
pub fn start() {
    // wasm-bindgen-rayon spawns Web Workers that re-instantiate this module, which would
    // re-run `start` and try to build a second event loop / GPU context. Worker globals
    // have no `window`, so bail out there and let the rayon worker entry take over.
    if web_sys::window().is_none() {
        return;
    }

    console_error_panic_hook::set_once();
    let _ = console_log::init_with_level(log::Level::Info);
    log::info!("dem-renderer web PoC starting");

    let gfx: Rc<RefCell<Option<Gfx>>> = Rc::new(RefCell::new(None));

    // The picker is a native <input type="file"> opened by the browser itself (via the
    // styled label in index.html). Synthesizing the click from a spawned future loses the
    // user activation and the browser suppresses the dialog — hence the native input.
    install_file_input(gfx.clone());

    let event_loop = EventLoop::new().expect("event loop");
    // On web the loop must not block; spawn_app returns immediately.
    use winit::platform::web::EventLoopExtWebSys;
    event_loop.spawn_app(App {
        gfx,
        initializing: false,
    });
}

/// Listen for the native `#file` input's `change` event (fired after the user picks a
/// file) and kick off the load.
fn install_file_input(gfx: Rc<RefCell<Option<Gfx>>>) {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;

    let document = web_sys::window()
        .and_then(|w| w.document())
        .expect("no document");
    let input: web_sys::HtmlInputElement = document
        .get_element_by_id("file")
        .expect("index.html must contain a #file input")
        .dyn_into()
        .expect("#file is not an <input>");

    let input_for_cb = input.clone();
    let cb = Closure::<dyn FnMut()>::new(move || {
        let Some(file) = input_for_cb.files().and_then(|f| f.get(0)) else {
            return;
        };
        // Clear the value so re-selecting the same file fires `change` again.
        input_for_cb.set_value("");
        wasm_bindgen_futures::spawn_local(load_file(file, gfx.clone()));
    });
    input
        .add_event_listener_with_callback("change", cb.as_ref().unchecked_ref())
        .expect("add change listener");
    cb.forget(); // keep the closure alive for the page lifetime
}

struct App {
    gfx: Rc<RefCell<Option<Gfx>>>,
    initializing: bool,
}

/// All GPU + scene state. Lives behind `Rc<RefCell<…>>` so the async GPU-init and
/// file-pick tasks can populate it after the event loop has started.
struct Gfx {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    ctx: GpuContext,
    scene: Option<GpuScene>,
    /// Set once a load is in flight; the redraw loop polls it for the worker result.
    load_rx: Option<Receiver<Loaded>>,
    // Fly camera (WASD + drag-look), mirroring the native viewer's yaw/pitch convention:
    // forward = [cos(p)·sin(y), -cos(p)·cos(y), sin(p)]; yaw=π looks +y (north), pitch<0 down.
    pos: [f32; 3],
    yaw: f32,
    pitch: f32,
    move_speed: f32, // m/s (×4 with Shift)
    keys: HashSet<KeyCode>,
    dragging: bool,
    last_cursor: Option<(f64, f64)>,
    last_t_ms: f64,
    sun_dir: [f32; 3],
    step_m: f32,
    t_max: f32,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.gfx.borrow().is_some() || self.initializing {
            return;
        }
        self.initializing = true;

        let attrs: WindowAttributes = {
            use winit::platform::web::WindowAttributesExtWebSys;
            WindowAttributes::default()
                .with_title("DEM Renderer (web)")
                .with_inner_size(winit::dpi::PhysicalSize::new(RENDER_W, RENDER_H))
                .with_append(true) // append the canvas to <body>
        };
        let window = Arc::new(el.create_window(attrs).expect("create window"));

        // Make the canvas focusable + focus it so it receives keyboard events on web
        // (winit delivers KeyboardInput only to the focused canvas).
        {
            use winit::platform::web::WindowExtWebSys;
            if let Some(canvas) = window.canvas() {
                let _ = canvas.set_attribute("tabindex", "0");
                let _ = canvas.focus();
            }
        }

        // GPU init is async on web — do it off the event-loop callback, then store Gfx.
        let slot = self.gfx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let ctx = GpuContext::new_async().await;
            let surface = ctx
                .instance
                .create_surface(window.clone())
                .expect("create surface");

            let caps = surface.get_capabilities(&ctx.adapter);
            let format = caps
                .formats
                .iter()
                .copied()
                .find(|f| *f == wgpu::TextureFormat::Bgra8Unorm)
                .unwrap_or(caps.formats[0]);
            let config = wgpu::SurfaceConfiguration {
                // COPY_DST so the compute output buffer can be blitted into the swapchain.
                usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::RENDER_ATTACHMENT,
                format,
                width: RENDER_W,
                height: RENDER_H,
                present_mode: wgpu::PresentMode::Fifo, // browsers don't expose Immediate
                alpha_mode: caps.alpha_modes[0],
                view_formats: vec![],
                desired_maximum_frame_latency: 2,
            };
            surface.configure(&ctx.device, &config);

            *slot.borrow_mut() = Some(Gfx {
                window: window.clone(),
                surface,
                ctx,
                scene: None,
                load_rx: None,
                pos: [0.0; 3],
                yaw: std::f32::consts::PI,
                pitch: -0.15,
                move_speed: 800.0,
                keys: HashSet::new(),
                dragging: false,
                last_cursor: None,
                last_t_ms: now_ms(),
                sun_dir: normalize3([0.4, 0.3, 0.85]),
                step_m: 30.0,
                t_max: 200_000.0,
            });
            log::info!("GPU ready — click the Open GeoTIFF button to choose a tile");
            window.request_redraw();
        });
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let mut guard = self.gfx.borrow_mut();
        let Some(gfx) = guard.as_mut() else { return };

        match event {
            WindowEvent::CloseRequested => el.exit(),

            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    match event.state {
                        ElementState::Pressed => {
                            gfx.keys.insert(code);
                        }
                        ElementState::Released => {
                            gfx.keys.remove(&code);
                        }
                    }
                    gfx.window.request_redraw(); // wake the render loop
                }
            }

            // Left-drag on the canvas to look around.
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                gfx.dragging = state == ElementState::Pressed;
                if !gfx.dragging {
                    gfx.last_cursor = None;
                }
                gfx.window.request_redraw();
            }

            WindowEvent::CursorMoved { position, .. } => {
                if gfx.dragging {
                    if let Some((lx, ly)) = gfx.last_cursor {
                        gfx.yaw += (position.x - lx) as f32 * 0.004;
                        gfx.pitch = (gfx.pitch - (position.y - ly) as f32 * 0.004).clamp(-1.5, 1.5);
                    }
                    gfx.window.request_redraw();
                }
                gfx.last_cursor = Some((position.x, position.y));
            }

            WindowEvent::RedrawRequested => {
                if gfx.scene.is_none()
                    && let Some(rx) = &gfx.load_rx
                    && let Ok(loaded) = rx.try_recv()
                {
                    gfx.build_scene(loaded);
                }
                gfx.update();
                gfx.render();
                // Keep animating ONLY while there's active input or a load in flight.
                // Otherwise stop so the GPU goes idle — a continuous full-screen raymarch
                // pins a weak GPU and stalls the whole macOS UI even at "low CPU".
                if !gfx.keys.is_empty() || gfx.dragging || gfx.load_rx.is_some() {
                    gfx.window.request_redraw();
                }
            }
            _ => {}
        }
    }
}

impl Gfx {
    /// Build the GPU scene from worker-produced CPU data. Shadow + AO are uniform (fully
    /// lit / unoccluded) for the PoC — the expensive DDA sweeps come later.
    fn build_scene(&mut self, loaded: Loaded) {
        let Loaded { hm, normals } = loaded;
        let n = hm.rows * hm.cols;
        let shadow = ShadowMask {
            data: vec![1.0; n],
            rows: hm.rows,
            cols: hm.cols,
        };
        let ao = vec![1.0_f32; n];

        // Camera: oblique bird's-eye from the south looking north across the tile.
        let dx = hm.dx_meters as f32;
        let dy = hm.dy_meters as f32;
        let w_m = hm.cols as f32 * dx;
        let h_m = hm.rows as f32 * dy;
        let max_h = hm
            .data
            .iter()
            .copied()
            .filter(|v| *v != hm.nodata)
            .fold(f32::NEG_INFINITY, f32::max);
        let max_h = if max_h.is_finite() { max_h } else { 1000.0 };
        let span = w_m.max(h_m);

        // Start the camera CLOSE to the terrain (inside the grid footprint), not staring
        // across the whole tile — long rays over the full extent are what pin a weak GPU.
        // Stand ~600 m above the centre cell, looking north and slightly down.
        let ci = (hm.rows / 2) * hm.cols + hm.cols / 2;
        let h_center = hm
            .data
            .get(ci)
            .copied()
            .filter(|v| *v != hm.nodata && v.is_finite())
            .unwrap_or(max_h * 0.5);
        self.pos = [w_m * 0.5, h_m * 0.5, h_center + 600.0];
        self.yaw = std::f32::consts::PI; // +y / north
        self.pitch = -0.15;
        // Scale fly speed to the tile so traversal feels consistent across resolutions.
        self.move_speed = (span * 0.01).clamp(200.0, 3000.0);
        // Step ~¼ cell near the camera (the shader grows it with distance via its LOD term).
        self.step_m = (dx / 4.0).max(1.0);
        // Cap the march distance so rays that miss the terrain (sky) don't iterate to 200 km
        // — a big per-frame cost on a weak GPU. ~80 km is plenty for a near-ground view.
        self.t_max = 80_000.0;

        log::info!(
            "scene: {}x{} cells, {:.0}x{:.0} m, max elev {:.0} m",
            hm.cols,
            hm.rows,
            w_m,
            h_m,
            max_h
        );

        self.scene = Some(GpuScene::new(
            self.ctx.clone(),
            &hm,
            &normals,
            &shadow,
            &ao,
            RENDER_W,
            RENDER_H,
        ));
        self.load_rx = None; // load consumed — allow picking another file
    }

    /// Apply WASD/Space/Shift movement for this frame (dt from the wall clock).
    fn update(&mut self) {
        let now = now_ms();
        let dt = (((now - self.last_t_ms) / 1000.0) as f32).clamp(0.0, 0.1);
        self.last_t_ms = now;
        if self.scene.is_none() {
            return;
        }

        let boost = if self.keys.contains(&KeyCode::ShiftLeft) {
            4.0
        } else {
            1.0
        };
        let v = self.move_speed * boost * dt;
        let (sy, cy) = (self.yaw.sin(), self.yaw.cos());
        let fwd_h = [sy, -cy]; // horizontal forward (unit)
        let right = [-cy, -sy]; // fwd_h × up
        let k = &self.keys;
        if k.contains(&KeyCode::KeyW) {
            self.pos[0] += fwd_h[0] * v;
            self.pos[1] += fwd_h[1] * v;
        }
        if k.contains(&KeyCode::KeyS) {
            self.pos[0] -= fwd_h[0] * v;
            self.pos[1] -= fwd_h[1] * v;
        }
        if k.contains(&KeyCode::KeyD) {
            self.pos[0] += right[0] * v;
            self.pos[1] += right[1] * v;
        }
        if k.contains(&KeyCode::KeyA) {
            self.pos[0] -= right[0] * v;
            self.pos[1] -= right[1] * v;
        }
        if k.contains(&KeyCode::Space) {
            self.pos[2] += v;
        }
        if k.contains(&KeyCode::KeyC) {
            self.pos[2] -= v;
        }
    }

    /// Camera target from yaw/pitch (matches the native viewer's forward convention).
    fn look_at(&self) -> [f32; 3] {
        let (sp, cp) = (self.pitch.sin(), self.pitch.cos());
        let (sy, cy) = (self.yaw.sin(), self.yaw.cos());
        let fwd = [cp * sy, -cp * cy, sp];
        [
            self.pos[0] + fwd[0],
            self.pos[1] + fwd[1],
            self.pos[2] + fwd[2],
        ]
    }

    fn render(&mut self) {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => {
                t
            }
            _ => return,
        };

        let mut encoder = self
            .ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });

        match &self.scene {
            // Nothing loaded yet — clear to a dark slate so the canvas is visibly alive.
            None => {
                let view = frame
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default());
                encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("clear"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            // Deliberately distinct from the page background so a teal
                            // canvas confirms GPU init + the render loop are alive (vs the
                            // CSS background showing through because nothing rendered).
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.0,
                                g: 0.45,
                                b: 0.5,
                                a: 1.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
            }
            Some(scene) => {
                scene.dispatch_frame(
                    &mut encoder,
                    self.pos,
                    self.look_at(),
                    70.0,
                    RENDER_W as f32 / RENDER_H as f32,
                    self.sun_dir,
                    self.step_m,
                    self.t_max,
                    0, // ao_mode
                    0, // shadows_enabled (uniform shadow buffer → off is correct)
                    0, // fog_enabled
                    0, // vat_mode
                    0, // lod_mode
                    0.0, // smooth_radius_m
                    0, // align_mode
                );
                encoder.copy_buffer_to_texture(
                    wgpu::TexelCopyBufferInfo {
                        buffer: scene.get_output_buffer(),
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(RENDER_W * 4),
                            rows_per_image: None,
                        },
                    },
                    frame.texture.as_image_copy(),
                    wgpu::Extent3d {
                        width: RENDER_W,
                        height: RENDER_H,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }

        self.ctx.queue.submit([encoder.finish()]);
        frame.present();
    }
}

/// Read the selected file's bytes via the File API, then offload parse + normals onto the
/// rayon worker pool. The result returns via an mpsc channel stored in `Gfx::load_rx`,
/// polled by the redraw loop.
async fn load_file(file: web_sys::File, slot: Rc<RefCell<Option<Gfx>>>) {
    match slot.borrow().as_ref() {
        None => {
            log::warn!("GPU not ready yet — pick again in a moment");
            return;
        }
        Some(g) if g.load_rx.is_some() => {
            log::info!("a load is already in progress");
            return;
        }
        _ => {}
    }

    let name = file.name();
    let buf = match wasm_bindgen_futures::JsFuture::from(file.array_buffer()).await {
        Ok(b) => b,
        Err(e) => {
            log::error!("file read failed: {e:?}");
            return;
        }
    };
    let bytes = js_sys::Uint8Array::new(&buf).to_vec();
    log::info!("read {} bytes from {name}", bytes.len());

    let (tx, rx) = std::sync::mpsc::channel::<Loaded>();
    if let Some(gfx) = slot.borrow_mut().as_mut() {
        gfx.load_rx = Some(rx);
        gfx.window.request_redraw();
    }

    // Runs on a SharedArrayBuffer-backed Web Worker via the global rayon pool.
    rayon::spawn(move || match load_from_bytes(bytes) {
        Ok(loaded) => {
            let _ = tx.send(loaded);
        }
        Err(e) => log::error!("GeoTIFF load failed: {e}"),
    });
}

fn load_from_bytes(bytes: Vec<u8>) -> Result<Loaded, String> {
    let mut hm = dem_io::parse_geotiff_auto_reader(std::io::Cursor::new(bytes))
        .map_err(|e| format!("{e:?}"))?;
    if hm.cols > MAX_TILE_DIM || hm.rows > MAX_TILE_DIM {
        let (oc, or) = (hm.cols, hm.rows);
        hm = center_crop(hm, MAX_TILE_DIM);
        log::info!("tile {oc}x{or} center-cropped to {}x{}", hm.cols, hm.rows);
    }
    let normals = terrain::compute_normals_vector(&hm);
    Ok(Loaded { hm, normals })
}

/// Center-crop a heightmap to at most `max`×`max` cells. CRS origins are left unchanged —
/// the PoC camera works in local metres from the grid corner, so the absolute georeference
/// offset doesn't matter here.
fn center_crop(hm: Heightmap, max: usize) -> Heightmap {
    let cols = hm.cols.min(max);
    let rows = hm.rows.min(max);
    let c0 = (hm.cols - cols) / 2;
    let r0 = (hm.rows - rows) / 2;
    let mut data = Vec::with_capacity(cols * rows);
    for r in 0..rows {
        let s = (r0 + r) * hm.cols + c0;
        data.extend_from_slice(&hm.data[s..s + cols]);
    }
    Heightmap {
        data,
        rows,
        cols,
        ..hm
    }
}

fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
    [v[0] / len, v[1] / len, v[2] / len]
}

/// Monotonic-ish wall clock in milliseconds (std::time::Instant panics on wasm).
fn now_ms() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now())
        .unwrap_or(0.0)
}
