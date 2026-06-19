//! `ViewerCore` — the platform-agnostic `winit::ApplicationHandler` that drives
//! the terrain viewer. Mirrors the binary's `viewer::Viewer` but with the
//! camera factored into [`FlyCamera`], the clock swapped to `web_time::Instant`,
//! and all off-thread work / tile I/O routed through the injected
//! [`Spawner`] / [`TileSource`] adapters. The shell builds the EventLoop,
//! window, surface and `GpuContext`, constructs a `ViewerCore`, and delegates
//! winit events to it.

use std::sync::{Arc, mpsc};

use dem_io::Heightmap;
use render_gpu::{GpuContext, GpuScene, VramClass};
use terrain::ShadowMask;
use web_time::Instant;

use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::WindowEvent,
    keyboard::KeyCode,
    window::{Window, WindowAttributes},
};

use crate::camera::FlyCamera;
use crate::consts::M_PER_DEG;
use crate::geo::{latlon_to_tile_metres, sun_position};
use crate::hud::HudRenderer;
use crate::platform::{Spawner, TileSource};
use crate::scene_build::{INIT_SIM_DAY, INIT_SIM_HOUR, compute_ao_cropped};
use crate::tiers::{
    AO_DRIFT_THRESHOLD_M, BevBaseState, TierRadii, cross_crs_world_origin_and_extent, tier_radii,
};
use crate::tile_index::TileIndex;

/// Pre-computed terrain data produced by the loading step (in the shell), handed
/// to [`ViewerCore::new`] so no loading work happens after construction.
pub struct PreparedScene {
    pub scene: GpuScene,
    pub hm: Arc<Heightmap>,
    pub lat_rad: f32,
    pub width: u32,
    pub height: u32,
    /// Overview cache built during loading (for projected high-res tiles).
    pub cache_path: Option<std::path::PathBuf>,
}

/// Initial camera placement (WGS84) and orientation. `Default` reproduces the
/// binary's hard-coded Hintertux-glacier start view.
#[derive(Clone, Copy, Debug)]
pub struct InitialView {
    pub cam_lat: f64,
    pub cam_lon: f64,
    pub cam_elev: f32,
    pub yaw: f32,
    pub pitch: f32,
}

impl Default for InitialView {
    fn default() -> Self {
        InitialView {
            cam_lat: 47.076211, // 47°04'34.36"N
            cam_lon: 11.687592, // 11°41'15.33"E
            cam_elev: 3258.0,
            yaw: (19627.0_f32).atan2(1718.0_f32),
            pitch: (-3472.0_f32).atan2(19702.0_f32),
        }
    }
}

/// Quality + behaviour settings, decoupled from the binary's launcher config.
pub struct ViewerSettings {
    pub vsync: bool,
    pub ao_mode: u32,
    pub shadows_enabled: bool,
    pub fog_enabled: bool,
    pub vat_mode: u32,
    pub lod_mode: u32,
    pub vram_class: VramClass,
    pub initial_view: InitialView,
}

/// How the viewer streams (or doesn't) tiles. Tile indices are built by the
/// shell (discovery is I/O) and passed in ready.
pub enum TierSetup {
    /// Static single tile — no sliding (geographic single-file mode).
    Static,
    /// Three-tier streaming over pre-built indices.
    Streaming {
        fine_index: Arc<TileIndex>,
        close_index: Arc<TileIndex>,
        base_index: Arc<TileIndex>,
        cam_lat: f64,
        cam_lon: f64,
    },
}

pub struct ViewerCore {
    scene: Option<GpuScene>,
    window: Option<Arc<Window>>,
    /// Surface handed over from the shell — reconfigured in resumed() instead of
    /// being recreated, which eliminates the visible flash during a transition.
    pre_surface: Option<wgpu::Surface<'static>>,
    surface: Option<wgpu::Surface<'static>>,
    surface_config: Option<wgpu::SurfaceConfiguration>,
    width: u32,
    height: u32,
    render_width: u32,
    vsync: bool,
    ao_mode: u32,
    shadows_enabled: bool,
    fog_enabled: bool,
    vat_mode: u32,        // 0=Ultra, 1=High, 2=Mid, 3=Low
    lod_mode: u32,        // 0=Ultra, 1=High, 2=Mid, 3=Low
    smooth_radius_m: f32, // close-range bicubic smoothing radius (f32::MAX = off)
    // fps counter
    fps_timer: Instant,
    frame_count: u32,
    fps: f64,
    // camera controls
    last_frame: Instant,
    camera: FlyCamera,
    // hud
    hud_renderer: Option<HudRenderer>,
    hud_visible: bool,
    // sun animation — date/time driven
    sim_day: i32,   // 1–365
    sim_hour: f32,  // 0.0–24.0 solar time
    lat_rad: f32,   // tile centre latitude (radians)
    day_accum: f32, // fractional day accumulator for [ / ] keys
    shadow_tx: mpsc::SyncSender<(f32, f32)>,
    shadow_rx: mpsc::Receiver<ShadowMask>,
    shadow_computing: bool,
    last_shadow_az: f32,
    last_shadow_el: f32,
    // drift-based AO recompute
    ao_tx: mpsc::SyncSender<(f64, f64)>,
    ao_rx: mpsc::Receiver<Vec<u8>>,
    ao_computing: bool,
    ao_last_x: f64, // tile-local metres of last AO centre
    ao_last_y: f64,
    // detail-tier speed gate: close/fine tiers are suppressed while flying fast
    detail_allowed_since: Option<Instant>,
    // base heightmap (shared with shadow worker; replaced on tile slide)
    hm: Arc<Heightmap>,
    bev_base: Option<BevBaseState>,
    tier_radii: TierRadii,
    close_tier_disabled: bool,
    fine_disabled_by_oom: bool,
    align_mode_viz: bool, // V key: show all 3 tiers as separate colored surfaces
    // Injected spawner — retained so a base-tier reload can respawn the shadow
    // and AO workers. The tile_source is consumed at construction (each tier
    // worker captures its own clone), so it is not stored.
    spawner: Arc<dyn Spawner>,
}

impl ViewerCore {
    /// Build a fully wired `ViewerCore` from a pre-loaded scene. Surface
    /// configuration and HUD setup happen later inside `resumed()`, which the
    /// shell calls immediately after this.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        window: Arc<Window>,
        surface: wgpu::Surface<'static>,
        prepared: PreparedScene,
        settings: ViewerSettings,
        setup: TierSetup,
        tile_source: Arc<dyn TileSource>,
        spawner: Arc<dyn Spawner>,
    ) -> Self {
        let PreparedScene {
            mut scene,
            hm,
            lat_rad,
            width,
            height,
            cache_path: _,
        } = prepared;
        let dx: f32 = scene.get_dx_meters();
        let dy: f32 = scene.get_dy_meters();

        let chosen_class = settings.vram_class;
        let radii = tier_radii(chosen_class);
        eprintln!(
            "[tier] vram_class={chosen_class:?} (adapter detected={:?}); \
             radii: base {:.0} km / drift {:.0} km, close {:.0} km / drift {:.0} km, fine {:.1} km / drift {:.1} km",
            scene.get_gpu_ctx().vram_class,
            radii.base_radius_m / 1000.0,
            radii.base_drift_m / 1000.0,
            radii.close_radius_m / 1000.0,
            radii.close_drift_m / 1000.0,
            radii.fine_radius_m / 1000.0,
            radii.fine_drift_m / 1000.0,
        );

        let iv = settings.initial_view;
        let init_cam_pos = latlon_to_tile_metres(iv.cam_lat, iv.cam_lon, &hm)
            .map(|(x, y)| [x, y, iv.cam_elev])
            .unwrap_or_else(|| {
                let cx = hm.cols as f32 * dx * 0.5;
                let cy = hm.rows as f32 * dy * 0.5;
                let center_i = (hm.rows / 2) * hm.cols + hm.cols / 2;
                let elev = hm
                    .data
                    .get(center_i)
                    .copied()
                    .filter(|&v| v > -1000.0)
                    .unwrap_or(1000.0);
                [cx, cy, elev + 2000.0]
            });
        let camera = FlyCamera::new(init_cam_pos, iv.yaw, iv.pitch);

        // shadow worker
        let (shadow_tx, worker_rx) = mpsc::sync_channel::<(f32, f32)>(1);
        let (worker_tx, shadow_rx) = mpsc::channel::<ShadowMask>();
        {
            let hm_w = Arc::clone(&hm);
            spawner.spawn(Box::new(move || {
                while let Ok((az, el)) = worker_rx.recv() {
                    let mask =
                        terrain::compute_shadow_vector_par_with_azimuth(&hm_w, az, el, 200.0);
                    if worker_tx.send(mask).is_err() {
                        break;
                    }
                }
            }));
        }

        // AO worker
        let (ao_tx, ao_worker_rx) = mpsc::sync_channel::<(f64, f64)>(1);
        let (ao_worker_tx, ao_rx) = mpsc::channel::<Vec<u8>>();
        {
            let hm_ao = Arc::clone(&hm);
            spawner.spawn(Box::new(move || {
                while let Ok((cam_x, cam_y)) = ao_worker_rx.recv() {
                    let ao = compute_ao_cropped(&hm_ao, cam_x, cam_y);
                    if ao_worker_tx.send(render_gpu::pack_ao_u8(&ao)).is_err() {
                        break;
                    }
                }
            }));
        }

        let bev_base: Option<BevBaseState> = match setup {
            TierSetup::Static => None,
            TierSetup::Streaming {
                fine_index,
                close_index,
                base_index,
                cam_lat,
                cam_lon,
            } => Some(BevBaseState::new(
                fine_index,
                close_index,
                base_index,
                cam_lat,
                cam_lon,
                lat_rad,
                radii,
                &hm,
                &mut scene,
                &tile_source,
                spawner.as_ref(),
            )),
        };

        ViewerCore {
            scene: Some(scene),
            window: Some(window),
            pre_surface: Some(surface),
            surface: None,
            surface_config: None,
            width,
            height,
            render_width: width,
            vsync: settings.vsync,
            ao_mode: settings.ao_mode,
            shadows_enabled: settings.shadows_enabled,
            fog_enabled: settings.fog_enabled,
            vat_mode: settings.vat_mode,
            lod_mode: settings.lod_mode,
            smooth_radius_m: 2000.0,
            fps_timer: Instant::now(),
            frame_count: 0,
            fps: 0.0,
            last_frame: Instant::now(),
            camera,
            hud_renderer: None,
            hud_visible: true,
            sim_day: INIT_SIM_DAY,
            sim_hour: INIT_SIM_HOUR,
            lat_rad,
            day_accum: 0.0,
            shadow_tx,
            shadow_rx,
            shadow_computing: false,
            last_shadow_az: 0.0,
            last_shadow_el: -1.0,
            ao_tx,
            ao_rx,
            ao_computing: false,
            ao_last_x: init_cam_pos[0] as f64,
            ao_last_y: init_cam_pos[1] as f64,
            detail_allowed_since: Some(Instant::now()),
            hm,
            bev_base,
            tier_radii: radii,
            close_tier_disabled: false,
            fine_disabled_by_oom: false,
            align_mode_viz: false,
            spawner,
        }
    }

    /// Poll the global OOM flag set by `device.on_uncaptured_error` and degrade
    /// the active tier set instead of letting the next allocation panic.
    ///
    /// Step-down order:
    ///  1. Fine tier — disabled, GPU memory reclaimed via `set_hm1m_inactive`.
    ///  2. Close tier — disabled, GPU memory reclaimed via `set_hm5m_inactive`.
    ///  3. If both already disabled and we still OOM, give up — the base tier
    ///     itself isn't safe to drop, so we let wgpu's default behaviour panic.
    fn poll_and_handle_oom(&mut self) {
        if !render_gpu::OOM_OBSERVED.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        render_gpu::clear_oom_flag();
        let count = render_gpu::OOM_COUNT.load(std::sync::atomic::Ordering::Relaxed);

        let Some(scene) = self.scene.as_mut() else {
            return;
        };

        // Step 1: kill fine tier if it's still alive.
        if let Some(ref mut bev_base) = self.bev_base
            && bev_base.fine.is_some()
        {
            eprintln!(
                "[OOM #{count}] disabling fine tier — freeing ~hm1m_tex + normal + shadow memory"
            );
            scene.set_hm1m_inactive();
            bev_base.fine = None;
            self.tier_radii.fine_radius_m = 0.0;
            self.tier_radii.fine_drift_m = 0.0;
            self.fine_disabled_by_oom = true;
            return;
        }

        // Step 2: kill close tier.
        if !self.close_tier_disabled {
            eprintln!(
                "[OOM #{count}] disabling close tier — freeing ~hm5m_tex + normal + shadow memory"
            );
            scene.set_hm5m_inactive();
            self.close_tier_disabled = true;
            self.tier_radii.close_radius_m = 0.0;
            self.tier_radii.close_drift_m = 0.0;
            return;
        }

        // Step 3: both detail tiers gone and the base tier won't fit. There's
        // no graceful path from here — keep running on whatever's currently
        // bound and let the user see the warning.
        eprintln!("[OOM #{count}] all detail tiers already disabled — base tier is the floor");
    }
}

/// Convert tile-local camera position (metres from top-left) to WGS84 (lat, lon).
fn cam_wgs84(cam_pos: [f32; 3], hm: &Heightmap) -> (f64, f64) {
    if dem_io::crs::is_geographic(&hm.crs_proj4) {
        // dx_meters is unreliable for geographic tiles: extract_window stores deg/px there,
        // while parse_geotiff and stitch_windows_geographic store actual m/px.
        // Always derive m/px from dx_deg (reliably deg/px in all code paths).
        let dx_m = hm.dx_deg * M_PER_DEG * hm.crs_origin_y.to_radians().cos();
        let dy_m = hm.dy_deg.abs() * M_PER_DEG;
        let px = cam_pos[0] as f64 / dx_m;
        let py = cam_pos[1] as f64 / dy_m;
        let lon = hm.crs_origin_x + px * hm.dx_deg;
        let lat = hm.crs_origin_y - py * hm.dy_deg.abs();
        (lat, lon)
    } else {
        let e = hm.crs_origin_x + cam_pos[0] as f64;
        let n = hm.crs_origin_y - cam_pos[1] as f64;
        dem_io::crs::to_wgs84(e, n, &hm.crs_proj4).unwrap_or((0.0, 0.0))
    }
}

impl ApplicationHandler for ViewerCore {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        // Reuse a pre-built window (from the shell) if one was provided.
        let window: Arc<Window> = if let Some(w) = &self.window {
            // The shell's event loop exit may have hidden the window on macOS.
            w.set_visible(true);
            w.focus_window();
            w.clone()
        } else {
            let w = Arc::new(
                event_loop
                    .create_window(
                        WindowAttributes::default()
                            .with_title("dem_renderer")
                            .with_inner_size(LogicalSize::new(self.width, self.height)),
                    )
                    .expect("error creating a window from event loop in resumed method call"),
            );
            self.window = Some(w.clone());
            w
        };

        // Sync dimensions with the actual window — the user may have resized the window
        // between pressing Start and the viewer initialising.  Do this before any GPU
        // allocations so the surface config, HUD, and scene buffer all use the right size.
        {
            let sz = window.inner_size();
            let actual_w = sz.width.max(1);
            let actual_h = sz.height.max(1);
            if actual_w != self.width || actual_h != self.height {
                self.width = actual_w;
                self.render_width = (actual_w + 63) & !63;
                self.height = actual_h;
                self.scene
                    .as_mut()
                    .unwrap()
                    .resize(self.render_width, actual_h);
            }
        }

        let scene: &GpuScene = self
            .scene
            .as_ref()
            .expect("no scene to get ctx for resumed method run");

        // Reuse the shell's surface if one was handed over — reconfiguring in-place
        // avoids the drop+recreate that would cause a visible flash during the transition.
        let surface = if let Some(s) = self.pre_surface.take() {
            s
        } else {
            scene
                .get_gpu_ctx()
                .instance
                .create_surface(window.clone())
                .expect("error creating a surface from default Instance in resumed method")
        };
        self.surface = Some(surface);

        // surface configuration
        let ctx: &GpuContext = scene.get_gpu_ctx();
        let adapter: &wgpu::Adapter = &ctx.adapter;
        let caps = self
            .surface
            .as_ref()
            .expect("no surface to get capabilities")
            .get_capabilities(adapter);
        let format = caps
            .formats
            .iter()
            .find(|&&f| f == wgpu::TextureFormat::Bgra8Unorm)
            .copied()
            .unwrap_or(caps.formats[0]);

        // HUD — created with the correct (possibly resized) dimensions.
        let hud_renderer: HudRenderer = HudRenderer::new(
            &scene.get_gpu_ctx().device,
            &scene.get_gpu_ctx().queue,
            self.width,
            self.height,
            format,
        );
        self.hud_renderer = Some(hud_renderer);

        let mut present_mode: wgpu::PresentMode = wgpu::PresentMode::Immediate;
        if self.vsync {
            present_mode = wgpu::PresentMode::Fifo;
        } else if !caps.present_modes.contains(&wgpu::PresentMode::Immediate) {
            present_mode = wgpu::PresentMode::Fifo;
            println!(
                "present mode in capabilities not fount: wgpu::PresentMode::Immediate; FALLBACK to wgpu::PresentMode::Fifo"
            )
        }

        let config: wgpu::SurfaceConfiguration = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: self.width,
            height: self.height,
            present_mode,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        let device: &wgpu::Device = &ctx.device;
        self.surface
            .as_ref()
            .expect("no surface to configure")
            .configure(device, &config);
        self.surface_config = Some(config);

        self.window
            .as_ref()
            .expect("no window for resumed method call")
            .request_redraw();
    }

    fn window_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => {
                // OOM safety net: react before any new allocation can be requested
                // this frame, so the degradation lands before the next reload tries.
                self.poll_and_handle_oom();

                // delta time for frame-rate-independent camera movement
                let dt = self.last_frame.elapsed().as_secs_f32();
                self.last_frame = Instant::now();

                // cam movements
                let cam_pos_before = self.camera.pos;
                self.camera.update(dt);

                // Keep the camera inside the loaded heightmap so the shader's bounds-check
                // never fires on the first ray step (which would render a solid blue frame).
                let hm_max_x = self.hm.cols as f32 * self.hm.dx_meters as f32 - 1.0;
                let hm_max_y = self.hm.rows as f32 * self.hm.dy_meters as f32 - 1.0;
                self.camera.pos[0] = self.camera.pos[0].clamp(1.0, hm_max_x);
                self.camera.pos[1] = self.camera.pos[1].clamp(1.0, hm_max_y);

                // Speed gate for close/fine tier triggers.
                // At boost speed (5000 m/s) loading a 20 km close-tier window is pointless —
                // the camera leaves before compute finishes. Gate on speed so triggers only
                // fire when the camera has been slow (< 2500 m/s) for 400 ms continuously.
                // Normal speed is 500 m/s, so the threshold sits cleanly between the two.
                const DETAIL_SPEED_GATE: f32 = 2500.0; // m/s
                const DETAIL_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(400);
                let cam_moved = {
                    let dx = self.camera.pos[0] - cam_pos_before[0];
                    let dy = self.camera.pos[1] - cam_pos_before[1];
                    (dx * dx + dy * dy).sqrt()
                };
                let cam_speed_est = cam_moved / dt.max(0.001);
                let was_fast = self.detail_allowed_since.is_none();
                if cam_speed_est > DETAIL_SPEED_GATE {
                    self.detail_allowed_since = None;
                } else if self.detail_allowed_since.is_none() {
                    self.detail_allowed_since = Some(Instant::now());
                }
                let is_fast = self.detail_allowed_since.is_none();
                if is_fast && !was_fast {
                    println!("camera moving fast ({cam_speed_est:.0} m/s) — detail suppressed");
                } else if !is_fast && was_fast {
                    println!("camera slowed ({cam_speed_est:.0} m/s) — debouncing");
                }
                let detail_allowed = self
                    .detail_allowed_since
                    .map(|t| t.elapsed() >= DETAIL_DEBOUNCE)
                    .unwrap_or(false);

                let look_at = self.camera.look_at();

                // advance time (+/-) and day ([ / ])
                let time_speed = if self.camera.speed_boost { 4.0_f32 } else { 0.4_f32 }; // hours/s
                let day_speed = if self.camera.speed_boost { 60.0_f32 } else { 10.0_f32 }; // days/s
                if self.camera.keys_held.contains(&KeyCode::Equal) {
                    self.sim_hour = (self.sim_hour + time_speed * dt).rem_euclid(24.0);
                }
                if self.camera.keys_held.contains(&KeyCode::Minus) {
                    self.sim_hour = (self.sim_hour - time_speed * dt).rem_euclid(24.0);
                }
                if self.camera.keys_held.contains(&KeyCode::BracketRight) {
                    self.day_accum += day_speed * dt;
                }
                if self.camera.keys_held.contains(&KeyCode::BracketLeft) {
                    self.day_accum -= day_speed * dt;
                }
                if self.day_accum.abs() >= 1.0 {
                    let steps = self.day_accum.trunc() as i32;
                    self.sim_day = (self.sim_day + steps - 1).rem_euclid(365) + 1;
                    self.day_accum = self.day_accum.fract();
                }

                // derive sun direction before acquiring scene borrow
                let (azimuth, elevation) = sun_position(self.lat_rad, self.sim_day, self.sim_hour);
                let r = elevation.cos();
                let sun_dir = [r * azimuth.sin(), -r * azimuth.cos(), elevation.sin()];

                // pick up finished shadow mask if ready
                if let Ok(new_mask) = self.shadow_rx.try_recv() {
                    self.scene
                        .as_ref()
                        .expect("no scene for shadow update")
                        .update_shadow(&new_mask);
                    self.shadow_computing = false;
                }

                // recompute shadow only when sun moves more than 0.1° (≈ 2 min real time at 0.4h/s)
                let sun_moved = (azimuth - self.last_shadow_az).abs() > 0.00175
                    || (elevation - self.last_shadow_el).abs() > 0.00175;
                if !self.shadow_computing
                    && elevation > 0.0
                    && sun_moved
                    && self.shadow_tx.try_send((azimuth, elevation)).is_ok()
                {
                    self.shadow_computing = true;
                    self.last_shadow_az = azimuth;
                    self.last_shadow_el = elevation;
                }

                // drift-based AO recompute (5 km threshold in tile-local metres)
                if let Ok(new_ao) = self.ao_rx.try_recv() {
                    self.scene.as_ref().unwrap().update_ao(&new_ao);
                    self.ao_computing = false;
                }
                if !self.ao_computing {
                    let cam_x = self.camera.pos[0] as f64;
                    let cam_y = self.camera.pos[1] as f64;
                    // recompute AO when camera drifts far enough that the 20km radius
                    // no longer fully covers the new position with good data
                    if ((cam_x - self.ao_last_x).abs() > AO_DRIFT_THRESHOLD_M
                        || (cam_y - self.ao_last_y).abs() > AO_DRIFT_THRESHOLD_M)
                        && self.ao_tx.try_send((cam_x, cam_y)).is_ok()
                    {
                        self.ao_computing = true;
                        self.ao_last_x = cam_x;
                        self.ao_last_y = cam_y;
                        println!("AO recompute triggered at ({cam_x:.0}, {cam_y:.0})");
                    }
                }

                // BEV two-tier drift reload
                if let Some(ref mut bev_base) = self.bev_base {
                    // ── base tier ──
                    if let Some(data) = bev_base.base.try_recv() {
                        // re-project camera to new heightmap tile-local metres
                        let (lat, lon) = cam_wgs84(self.camera.pos, &self.hm);
                        if let Some((nx, ny)) = latlon_to_tile_metres(lat, lon, &data.hm) {
                            self.camera.pos[0] = nx;
                            self.camera.pos[1] = ny;
                        } else {
                            // Camera drifted past the new tile's extent while loading.
                            // Place at tile centre so the shader never fires an all-blue frame.
                            let (tile_w, tile_h) = if dem_io::crs::is_geographic(&data.hm.crs_proj4)
                            {
                                let dx_m = (data.hm.dx_deg
                                    * M_PER_DEG
                                    * data.hm.crs_origin_y.to_radians().cos())
                                    as f32;
                                let dy_m = (data.hm.dy_deg.abs() * M_PER_DEG) as f32;
                                (data.hm.cols as f32 * dx_m, data.hm.rows as f32 * dy_m)
                            } else {
                                (
                                    data.hm.cols as f32 * data.hm.dx_meters as f32,
                                    data.hm.rows as f32 * data.hm.dy_meters as f32,
                                )
                            };
                            println!(
                                "WARN: camera ({lat:.4}°, {lon:.4}°) outside new base tile — snapping to centre"
                            );
                            self.camera.pos[0] = (tile_w * 0.5).clamp(1.0, tile_w - 1.0);
                            self.camera.pos[1] = (tile_h * 0.5).clamp(1.0, tile_h - 1.0);
                        }
                        {
                            let scene = self.scene.as_mut().unwrap();
                            scene.update_heightmap(
                                &data.hm,
                                &data.gpu_hm_f16,
                                &data.gpu_hm_mips,
                                &data.gpu_normals_u32,
                                &data.gpu_ao_u8,
                            );
                            scene.update_shadow(&data.shadow);
                            // The fine-tier origins are offsets relative to the base heightmap origin.
                            // After a base reload the origin shifts, so the old offsets are wrong —
                            // hide both fine tiers until their workers deliver fresh windows.
                            scene.set_hm5m_inactive();
                            scene.set_hm1m_inactive();
                        }
                        self.hm = data.hm;
                        // Recalibrate drift threshold to match the actual loaded window.
                        let new_half_m = (self.hm.cols as f64 * self.hm.dx_meters)
                            .min(self.hm.rows as f64 * self.hm.dy_meters)
                            * 0.5;
                        let new_thresh = self.tier_radii.base_drift_m.min(new_half_m * 0.5);
                        // Geographic base tracks position in degrees; convert the metre threshold.
                        let new_thresh_unit = if dem_io::crs::is_geographic(&self.hm.crs_proj4) {
                            new_thresh / M_PER_DEG
                        } else {
                            new_thresh
                        };
                        bev_base.base.update_threshold(new_thresh_unit);
                        // close and fine tier offsets were relative to the old base origin — must reload
                        bev_base.close.invalidate();
                        if let Some(ref mut fine) = bev_base.fine {
                            fine.invalidate();
                        }
                        // Respawn shadow worker with updated heightmap.
                        let (new_tx, new_worker_rx) = mpsc::sync_channel::<(f32, f32)>(1);
                        let (new_worker_tx, new_rx) = mpsc::channel::<ShadowMask>();
                        let old_tx = std::mem::replace(&mut self.shadow_tx, new_tx);
                        let _ = std::mem::replace(&mut self.shadow_rx, new_rx);
                        drop(old_tx);
                        self.shadow_computing = false;
                        let hm_w = Arc::clone(&self.hm);
                        self.spawner.spawn(Box::new(move || {
                            while let Ok((az, el)) = new_worker_rx.recv() {
                                let mask = terrain::compute_shadow_vector_par_with_azimuth(
                                    &hm_w, az, el, 200.0,
                                );
                                if new_worker_tx.send(mask).is_err() {
                                    break;
                                }
                            }
                        }));
                        // Respawn AO worker with updated heightmap so AO data matches the new
                        // tile's dimensions and terrain layout.
                        let (new_ao_tx, new_ao_worker_rx) = mpsc::sync_channel::<(f64, f64)>(1);
                        let (new_ao_worker_tx, new_ao_rx) = mpsc::channel::<Vec<u8>>();
                        let old_ao_tx = std::mem::replace(&mut self.ao_tx, new_ao_tx);
                        let _ = std::mem::replace(&mut self.ao_rx, new_ao_rx);
                        drop(old_ao_tx);
                        self.ao_computing = false;
                        // Force an immediate AO recompute for the new tile centre.
                        self.ao_last_x = f64::MAX;
                        self.ao_last_y = f64::MAX;
                        let hm_ao = Arc::clone(&self.hm);
                        self.spawner.spawn(Box::new(move || {
                            while let Ok((cam_x, cam_y)) = new_ao_worker_rx.recv() {
                                let ao = compute_ao_cropped(&hm_ao, cam_x, cam_y);
                                if new_ao_worker_tx.send(render_gpu::pack_ao_u8(&ao)).is_err() {
                                    break;
                                }
                            }
                        }));
                        println!(
                            "BEV base reloaded: {}×{} at {:.1}m/px",
                            self.hm.cols, self.hm.rows, self.hm.dx_meters
                        );
                    }
                    // Always check base drift even while a reload is in-flight.
                    {
                        let (lat, lon) = cam_wgs84(self.camera.pos, &self.hm);
                        if !bev_base.base.computing
                            && bev_base.base.needs_reload(lat, lon)
                            && bev_base.base.try_trigger(lat, lon)
                        {
                            println!("BEV base reload triggered at lat={lat:.4} lon={lon:.4}");
                        }
                    }

                    // ── 5 m close tier ──
                    if let Some(data) = bev_base.close.try_recv() {
                        // After an OOM-driven shutdown the worker may still
                        // deliver one in-flight reload — drop it on the floor
                        // instead of re-allocating the texture we just freed.
                        if self.close_tier_disabled {
                            eprintln!("[OOM] discarding in-flight close reload (tier disabled)");
                        } else {
                            let (origin_x, origin_y, extent_x, extent_y, rot_rad) =
                                cross_crs_world_origin_and_extent(&data.hm, &self.hm);
                            self.scene.as_mut().unwrap().upload_hm5m(
                                origin_x,
                                origin_y,
                                rot_rad,
                                extent_x,
                                extent_y,
                                &data.hm,
                                &data.gpu_normals_rg16,
                                &data.shadow,
                            );
                            println!(
                                "5m tier updated: {}×{} at {:.1}m/px",
                                data.hm.cols, data.hm.rows, data.hm.dx_meters
                            );
                        }
                    }
                    if detail_allowed && !bev_base.close.computing && !self.close_tier_disabled {
                        let (lat, lon) = cam_wgs84(self.camera.pos, &self.hm);
                        if bev_base.close.needs_reload(lat, lon)
                            && bev_base.close.try_trigger(lat, lon)
                        {
                            println!("5m reload triggered at lat={lat:.4} lon={lon:.4}");
                        }
                    }

                    // ── 1 m fine tier ──
                    if let Some(ref mut fine) = bev_base.fine {
                        if let Some(data) = fine.try_recv() {
                            let (origin_x, origin_y, extent_x, extent_y, rot_rad) =
                                cross_crs_world_origin_and_extent(&data.hm, &self.hm);
                            self.scene.as_mut().unwrap().upload_hm1m(
                                origin_x,
                                origin_y,
                                rot_rad,
                                extent_x,
                                extent_y,
                                &data.hm,
                                &data.gpu_normals_rg16,
                                &data.shadow,
                            );
                            println!(
                                "1m tier updated: {}×{} at {:.1}m/px",
                                data.hm.cols, data.hm.rows, data.hm.dx_meters
                            );
                        }
                        if detail_allowed && !fine.computing {
                            let (lat, lon) = cam_wgs84(self.camera.pos, &self.hm);
                            if fine.needs_reload(lat, lon) && fine.try_trigger(lat, lon) {
                                println!("1m reload triggered at lat={lat:.4} lon={lon:.4}");
                            }
                        }
                    }
                }

                let surface: &wgpu::Surface =
                    self.surface.as_ref().expect("no surface for window event");
                let scene: &GpuScene = self.scene.as_ref().expect("no scene for window event");
                let ctx: &GpuContext = scene.get_gpu_ctx();
                let surface_texture = match surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(t) => t,
                    wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
                    _ => return, // Timeout or occluded — skip this frame
                };

                let mut encoder =
                    ctx.device
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("blit_enc"),
                        });

                let vat_step_divisors = [20.0_f32, 10.0, 5.0, 3.0];
                let step_m = scene.get_dx_meters() / vat_step_divisors[self.vat_mode as usize];
                scene.dispatch_frame(
                    &mut encoder,
                    self.camera.pos,
                    look_at,
                    70.0,
                    self.width as f32 / self.height as f32,
                    sun_dir,
                    step_m,
                    200_000.0,
                    self.ao_mode,
                    self.shadows_enabled as u32,
                    self.fog_enabled as u32,
                    self.vat_mode,
                    self.lod_mode,
                    self.smooth_radius_m,
                    self.align_mode_viz as u32,
                );
                let output_buf: &wgpu::Buffer = scene.get_output_buffer();

                encoder.copy_buffer_to_texture(
                    wgpu::TexelCopyBufferInfo {
                        buffer: output_buf,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(self.render_width * 4), // 4 bytes per RGBA pixel
                            rows_per_image: None,
                        },
                    },
                    surface_texture.texture.as_image_copy(),
                    wgpu::Extent3d {
                        width: self.width,
                        height: self.height,
                        depth_or_array_layers: 1,
                    },
                );

                // HUD
                if self.hud_visible {
                    let surface_view = surface_texture
                        .texture
                        .create_view(&wgpu::TextureViewDescriptor::default());
                    let hud = self.hud_renderer.as_mut().expect("no hud renderer");
                    hud.set_oom_state(self.fine_disabled_by_oom, self.close_tier_disabled);
                    hud.draw(
                        &scene.get_gpu_ctx().queue,
                        &scene.get_gpu_ctx().device,
                        &mut encoder,
                        &surface_view,
                        self.fps as f32,
                        1000.0,
                        self.sim_day,
                        self.sim_hour,
                        self.ao_mode,
                        self.shadows_enabled,
                        self.fog_enabled,
                        self.vat_mode,
                        self.lod_mode,
                        self.smooth_radius_m,
                    );
                }

                ctx.queue.submit([encoder.finish()]);
                surface_texture.present();

                // fps counter
                self.frame_count += 1;
                let elapsed = self.fps_timer.elapsed().as_secs_f64();
                if elapsed >= 1.0 {
                    self.fps = self.frame_count as f64 / elapsed;
                    self.fps_timer = Instant::now();
                    self.frame_count = 0;
                    self.window.as_ref().unwrap().set_title(&format!(
                        "dem_renderer  {:.0} fps  {:.1} ms",
                        self.fps,
                        1000.0 / self.fps
                    ));
                }

                self.window
                    .as_ref()
                    .expect("no window for window event")
                    .request_redraw();
            }
            WindowEvent::KeyboardInput {
                device_id: _,
                event,
                is_synthetic: _,
            } => {
                if let winit::keyboard::PhysicalKey::Code(kc) = event.physical_key {
                    if kc == KeyCode::KeyQ && event.state == winit::event::ElementState::Pressed {
                        if !self.camera.immersive_mode {
                            self.camera.immersive_mode = true;
                            let _ = self
                                .window
                                .as_ref()
                                .unwrap()
                                .set_cursor_grab(winit::window::CursorGrabMode::Locked);
                            self.window.as_ref().unwrap().set_cursor_visible(false);
                        } else {
                            self.camera.immersive_mode = false;
                            let _ = self
                                .window
                                .as_ref()
                                .unwrap()
                                .set_cursor_grab(winit::window::CursorGrabMode::None);
                            self.window.as_ref().unwrap().set_cursor_visible(true);
                        }

                        return;
                    }
                    if kc == KeyCode::KeyE && event.state == winit::event::ElementState::Pressed {
                        self.hud_visible = !self.hud_visible;
                        return;
                    }
                    if kc == KeyCode::Slash && event.state == winit::event::ElementState::Pressed {
                        self.ao_mode = (self.ao_mode + 1).rem_euclid(6);
                        return;
                    }
                    if kc == KeyCode::Period && event.state == winit::event::ElementState::Pressed {
                        self.shadows_enabled = !self.shadows_enabled;
                        return;
                    }
                    if kc == KeyCode::Comma && event.state == winit::event::ElementState::Pressed {
                        self.fog_enabled = !self.fog_enabled;
                        return;
                    }
                    if kc == KeyCode::Semicolon
                        && event.state == winit::event::ElementState::Pressed
                    {
                        self.vat_mode = (self.vat_mode + 1).rem_euclid(4);
                        return;
                    }
                    if kc == KeyCode::Quote && event.state == winit::event::ElementState::Pressed {
                        self.lod_mode = (self.lod_mode + 1).rem_euclid(4);
                        return;
                    }
                    if kc == KeyCode::KeyV && event.state == winit::event::ElementState::Pressed {
                        self.align_mode_viz = !self.align_mode_viz;
                        return;
                    }
                    if kc == KeyCode::KeyB && event.state == winit::event::ElementState::Pressed {
                        // 0.0 = off (dist < 0 never true), other values = active radius
                        let presets = [0.0_f32, 500.0, 1000.0, 2000.0, 5000.0];
                        let cur = presets
                            .iter()
                            .position(|&r| r >= self.smooth_radius_m)
                            .unwrap_or(0);
                        self.smooth_radius_m = presets[(cur + 1) % presets.len()];
                        return;
                    }
                    // Debug: force close + fine tier reloads on the next frame.
                    // Used to repro tier-swap memory peaks without flying.
                    if kc == KeyCode::KeyR && event.state == winit::event::ElementState::Pressed {
                        if let Some(ref mut bev_base) = self.bev_base {
                            bev_base.close.invalidate();
                            if let Some(ref mut fine) = bev_base.fine {
                                fine.invalidate();
                            }
                            eprintln!("[vram] debug: close + fine tiers invalidated (R)");
                        }
                        return;
                    }
                    // Debug: simulate a wgpu OOM event. Used to test the
                    // degradation path on machines that don't actually OOM
                    // (M4 Max, high-VRAM cards). Each press steps down one
                    // tier (fine → close → no-op).
                    if kc == KeyCode::KeyO && event.state == winit::event::ElementState::Pressed {
                        render_gpu::signal_oom_for_testing();
                        eprintln!("[vram] debug: simulated OOM (O)");
                        return;
                    }
                    if kc == KeyCode::ShiftLeft {
                        match event.state {
                            winit::event::ElementState::Pressed => {
                                self.camera.speed_boost = true;
                            }
                            winit::event::ElementState::Released => {
                                self.camera.speed_boost = false;
                            }
                        }
                        return;
                    }

                    match event.state {
                        winit::event::ElementState::Pressed => self.camera.keys_held.insert(kc),
                        winit::event::ElementState::Released => self.camera.keys_held.remove(&kc),
                    };
                }
            }
            WindowEvent::MouseInput { state, button, .. }
                if button == winit::event::MouseButton::Left && !self.camera.immersive_mode =>
            {
                match state {
                    winit::event::ElementState::Pressed => {
                        self.camera.mouse_look = true;
                        let _ = self
                            .window
                            .as_ref()
                            .unwrap()
                            .set_cursor_grab(winit::window::CursorGrabMode::Locked);
                        self.window.as_ref().unwrap().set_cursor_visible(false);
                    }
                    winit::event::ElementState::Released => {
                        self.camera.mouse_look = false;
                        let _ = self
                            .window
                            .as_ref()
                            .unwrap()
                            .set_cursor_grab(winit::window::CursorGrabMode::None);
                        self.window.as_ref().unwrap().set_cursor_visible(true);
                    }
                }
            }
            WindowEvent::Resized(new_size) => {
                // 1. guard against zero-size (happens on minimize on some platforms)
                if new_size.width == 0 || new_size.height == 0 {
                    return;
                }

                // 2. update stored dimensions
                self.width = new_size.width;
                self.render_width = (new_size.width + 63) & !63;
                self.height = new_size.height;

                // 3. reconfigure the surface
                if let (Some(surface), Some(cfg), Some(scene)) =
                    (&self.surface, &mut self.surface_config, &mut self.scene)
                {
                    cfg.width = new_size.width;
                    cfg.height = new_size.height;
                    surface.configure(&scene.get_gpu_ctx().device, cfg);

                    // 4. reallocate output buffer in GpuScene
                    // surface.configure keeps using self.width (actual)
                    scene.resize(self.render_width, self.height);
                }

                // update hint hud
                self.hud_renderer
                    .as_mut()
                    .expect("no hud renderer")
                    .update_size(
                        &self
                            .scene
                            .as_ref()
                            .expect("no scene for hud resize")
                            .get_gpu_ctx()
                            .queue,
                        new_size.width,
                        new_size.height,
                    );
            }
            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &winit::event_loop::ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        event: winit::event::DeviceEvent,
    ) {
        if let winit::event::DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            self.camera.apply_mouse_delta(dx, dy);
        }
    }
}
