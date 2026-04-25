mod hud_renderer;
use std::sync::mpsc;
use std::{path::Path, sync::Arc};

use dem_io::Heightmap;
use render_gpu::{GpuContext, GpuScene};
use terrain::ShadowMask;

use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::WindowEvent,
    event_loop::EventLoop,
    keyboard::KeyCode,
    window::{Window, WindowAttributes},
};

use crate::viewer::hud_renderer::HudRenderer;

struct Viewer {
    scene: Option<GpuScene>,
    window: Option<Arc<Window>>,
    surface: Option<wgpu::Surface<'static>>,
    surface_config: Option<wgpu::SurfaceConfiguration>,
    width: u32,
    height: u32,
    render_width: u32,
    vsync: bool,
    ao_mode: u32,
    shadows_enabled: bool,
    fog_enabled: bool,
    vat_mode: u32, // 0=Ultra, 1=High, 2=Mid, 3=Low
    lod_mode: u32, // 0=Ultra, 1=High, 2=Mid, 3=Low
    // fps counter
    fps_timer: std::time::Instant,
    frame_count: u32,
    fps: f64,
    // camera controls
    last_frame: std::time::Instant,
    cam_pos: [f32; 3],
    yaw: f32,
    pitch: f32,
    keys_held: std::collections::HashSet<winit::keyboard::KeyCode>,
    mouse_look: bool,
    immersive_mode: bool,
    speed_boost: bool,
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
}

impl ApplicationHandler for Viewer {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        let window: Arc<Window> = Arc::new(
            event_loop
                .create_window(
                    WindowAttributes::default()
                        .with_title("dem_renderer")
                        .with_inner_size(LogicalSize::new(self.width, self.height)),
                )
                .expect("error creating a window from event loop in resumed method call"),
        );

        self.window = Some(window.clone());

        let scene: &GpuScene = &self
            .scene
            .as_ref()
            .expect("no scene to get ctx for resumed method run");

        let instance: &wgpu::Instance = &scene.get_gpu_ctx().instance;

        self.surface = Some(
            instance
                .create_surface(window.clone())
                .expect("error creating a surface from default Instance in resumed method"),
        );

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

        // HUD
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
            println!("present mode in capabilities not fount: wgpu::PresentMode::Immediate; FALLBACK to wgpu::PresentMode::Fifo")
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
        window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => {
                self.last_frame = std::time::Instant::now();
                let surface: &wgpu::Surface =
                    self.surface.as_ref().expect("no surface for window event");
                let scene: &GpuScene = self.scene.as_ref().expect("no scene for window event");
                let ctx: &GpuContext = scene.get_gpu_ctx();
                let surface_texture = match surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(t) => t,
                    wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
                    _ => return, // Timeout or occluded — skip this frame
                };

                // cam movements
                // delta time for frame-rate-independent movement
                let dt = self.last_frame.elapsed().as_secs_f32();
                let speed_boost_value = if self.speed_boost { 10.0 } else { 1.0 };
                let speed = 500.0_f32 * speed_boost_value; // meters per second

                // horizontal movement vectors from yaw only
                let forward_h = [self.yaw.sin(), -self.yaw.cos(), 0.0_f32];
                let right_h = [self.yaw.cos(), self.yaw.sin(), 0.0_f32];

                if self.keys_held.contains(&KeyCode::KeyW) {
                    self.cam_pos[0] += forward_h[0] * speed * dt;
                    self.cam_pos[1] += forward_h[1] * speed * dt;
                }
                if self.keys_held.contains(&KeyCode::KeyS) {
                    self.cam_pos[0] -= forward_h[0] * speed * dt;
                    self.cam_pos[1] -= forward_h[1] * speed * dt;
                }
                if self.keys_held.contains(&KeyCode::KeyA) {
                    self.cam_pos[0] -= right_h[0] * speed * dt;
                    self.cam_pos[1] -= right_h[1] * speed * dt;
                }
                if self.keys_held.contains(&KeyCode::KeyD) {
                    self.cam_pos[0] += right_h[0] * speed * dt;
                    self.cam_pos[1] += right_h[1] * speed * dt;
                }
                if self.keys_held.contains(&KeyCode::Space) {
                    self.cam_pos[2] += speed * dt;
                }
                if self.keys_held.contains(&KeyCode::ShiftLeft) {
                    self.cam_pos[2] -= speed * dt;
                }

                // full forward vector with pitch for look_at
                let fwd = [
                    self.pitch.cos() * self.yaw.sin(),
                    -self.pitch.cos() * self.yaw.cos(),
                    self.pitch.sin(),
                ];
                let look_at = [
                    self.cam_pos[0] + fwd[0],
                    self.cam_pos[1] + fwd[1],
                    self.cam_pos[2] + fwd[2],
                ];

                // advance time (+/-) and day ([ / ])
                let time_speed = if self.speed_boost { 4.0_f32 } else { 0.4_f32 }; // hours/s
                let day_speed = if self.speed_boost { 60.0_f32 } else { 10.0_f32 }; // days/s
                if self.keys_held.contains(&KeyCode::Equal) {
                    self.sim_hour = (self.sim_hour + time_speed * dt).rem_euclid(24.0);
                }
                if self.keys_held.contains(&KeyCode::Minus) {
                    self.sim_hour = (self.sim_hour - time_speed * dt).rem_euclid(24.0);
                }
                if self.keys_held.contains(&KeyCode::BracketRight) {
                    self.day_accum += day_speed * dt;
                }
                if self.keys_held.contains(&KeyCode::BracketLeft) {
                    self.day_accum -= day_speed * dt;
                }
                if self.day_accum.abs() >= 1.0 {
                    let steps = self.day_accum.trunc() as i32;
                    self.sim_day = (self.sim_day + steps - 1).rem_euclid(365) + 1;
                    self.day_accum = self.day_accum.fract();
                }

                // derive sun direction from geographic solar position
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

                // kick off next shadow recompute if worker is free and sun is above horizon
                if !self.shadow_computing && elevation > 0.0 {
                    if self.shadow_tx.try_send((azimuth, elevation)).is_ok() {
                        self.shadow_computing = true;
                    }
                }

                let mut encoder =
                    ctx.device
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("blit_enc"),
                        });

                let vat_step_divisors = [20.0_f32, 10.0, 5.0, 3.0];
                let step_m = scene.get_dx_meters() / vat_step_divisors[self.vat_mode as usize];
                scene.dispatch_frame(
                    &mut encoder,
                    self.cam_pos,
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
                );
                let output_buf: &wgpu::Buffer = scene.get_output_buffer();

                encoder.copy_buffer_to_texture(
                    wgpu::TexelCopyBufferInfo {
                        buffer: &output_buf,
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
                    self.hud_renderer.as_mut().expect("no hud renderer").draw(
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
                    );
                }

                ctx.queue.submit([encoder.finish()]);
                surface_texture.present();

                // fps counter
                self.frame_count += 1;
                let elapsed = self.fps_timer.elapsed().as_secs_f64();
                if elapsed >= 1.0 {
                    self.fps = self.frame_count as f64 / elapsed;
                    self.fps_timer = std::time::Instant::now();
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
                        if !self.immersive_mode {
                            self.immersive_mode = true;
                            let _ = self
                                .window
                                .as_ref()
                                .unwrap()
                                .set_cursor_grab(winit::window::CursorGrabMode::Locked);
                            self.window.as_ref().unwrap().set_cursor_visible(false);
                        } else {
                            self.immersive_mode = false;
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
                    if kc == KeyCode::SuperLeft || kc == KeyCode::AltLeft {
                        match event.state {
                            winit::event::ElementState::Pressed => {
                                self.speed_boost = true;
                            }
                            winit::event::ElementState::Released => {
                                self.speed_boost = false;
                            }
                        }
                        return;
                    }
                    match event.state {
                        winit::event::ElementState::Pressed => self.keys_held.insert(kc),
                        winit::event::ElementState::Released => self.keys_held.remove(&kc),
                    };
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if button == winit::event::MouseButton::Left && !self.immersive_mode {
                    match state {
                        winit::event::ElementState::Pressed => {
                            self.mouse_look = true;
                            let _ = self
                                .window
                                .as_ref()
                                .unwrap()
                                .set_cursor_grab(winit::window::CursorGrabMode::Locked);
                            self.window.as_ref().unwrap().set_cursor_visible(false);
                        }
                        winit::event::ElementState::Released => {
                            self.mouse_look = false;
                            let _ = self
                                .window
                                .as_ref()
                                .unwrap()
                                .set_cursor_grab(winit::window::CursorGrabMode::None);
                            self.window.as_ref().unwrap().set_cursor_visible(true);
                        }
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
            if !self.mouse_look && !self.immersive_mode {
                return;
            }
            let sensitivity = 0.001_f32;
            let mut direction_inversion: f32 = 1.0;
            if !self.immersive_mode {
                direction_inversion = -1.0;
            }
            self.yaw += dx as f32 * sensitivity * direction_inversion;
            self.pitch -= dy as f32 * sensitivity * direction_inversion;
            self.pitch = self.pitch.clamp(-1.57, 1.57);
        }
    }
}

/// Lambert Conformal Conic forward projection for EPSG:31287 (MGI / Austria Lambert).
/// Input: WGS84 lat/lon in degrees. Output: (easting, northing) in metres.
fn lcc_epsg31287(lat_deg: f64, lon_deg: f64) -> (f64, f64) {
    // Bessel 1841 ellipsoid
    let a = 6_377_397.155_f64;
    let f = 1.0 / 299.152_812_8_f64;
    let e2 = 2.0 * f - f * f;
    let e = e2.sqrt();

    let to_rad = std::f64::consts::PI / 180.0;
    let lat = lat_deg * to_rad;
    let lon = lon_deg * to_rad;

    // EPSG:31287 defining parameters
    let lat0 = 47.5 * to_rad; // latitude of false origin
    let lon0 = 13.333_333 * to_rad; // central meridian
    let lat1 = 49.0 * to_rad; // standard parallel 1
    let lat2 = 46.0 * to_rad; // standard parallel 2
    let fe = 400_000.0_f64; // false easting
    let fn_ = 400_000.0_f64; // false northing

    // Helper: m (Snyder eq 15-11)
    let m = |phi: f64| {
        let sin_phi = phi.sin();
        phi.cos() / (1.0 - e2 * sin_phi * sin_phi).sqrt()
    };
    // Helper: t (Snyder eq 15-9)
    let t = |phi: f64| {
        let sin_phi = phi.sin();
        let e_sin = e * sin_phi;
        ((1.0 - sin_phi) / (1.0 + sin_phi) * ((1.0 + e_sin) / (1.0 - e_sin)).powf(e)).sqrt()
    };

    let m1 = m(lat1);
    let m2 = m(lat2);
    let t0 = t(lat0);
    let t1 = t(lat1);
    let t2 = t(lat2);

    let n = (m1.ln() - m2.ln()) / (t1.ln() - t2.ln());
    let big_f = m1 / (n * t1.powf(n));
    let rho0 = a * big_f * t0.powf(n);
    let rho = a * big_f * t(lat).powf(n);
    let theta = n * (lon - lon0);

    let easting = fe + rho * theta.sin();
    let northing = fn_ + rho0 - rho * theta.cos();
    (easting, northing)
}

/// Spherical LAEA forward projection for EPSG:3035 (ETRS89 / LAEA Europe).
/// Input: WGS84 lat/lon degrees. Output: (easting, northing) metres.
fn laea_epsg3035(lat_deg: f64, lon_deg: f64) -> (f64, f64) {
    let r = 6_371_000.0_f64;
    let lat = lat_deg.to_radians();
    let lon = lon_deg.to_radians();
    let lat0 = 52.0_f64.to_radians();
    let lon0 = 10.0_f64.to_radians();
    let fe = 4_321_000.0_f64;
    let fn_ = 3_210_000.0_f64;

    let k =
        (2.0 / (1.0 + lat0.sin() * lat.sin() + lat0.cos() * lat.cos() * (lon - lon0).cos())).sqrt();
    let easting = fe + r * k * lat.cos() * (lon - lon0).sin();
    let northing =
        fn_ + r * k * (lat0.cos() * lat.sin() - lat0.sin() * lat.cos() * (lon - lon0).cos());
    (easting, northing)
}

/// Convert WGS84 lat/lon to tile-local metres (cam_pos.x, cam_pos.y).
/// Returns None if the position falls outside the tile bounds.
fn latlon_to_tile_metres(lat: f64, lon: f64, hm: &Heightmap) -> Option<(f32, f32)> {
    let (x, y) = match hm.crs_epsg {
        31287 => {
            let (easting, northing) = lcc_epsg31287(lat, lon);
            (easting - hm.crs_origin_x, hm.crs_origin_y - northing)
        }
        3035 => {
            let (easting, northing) = laea_epsg3035(lat, lon);
            (easting - hm.crs_origin_x, hm.crs_origin_y - northing)
        }
        _ => {
            // Geographic (EPSG:4326)
            let x = (lon - hm.crs_origin_x) / hm.dx_deg * hm.dx_meters;
            let y = (hm.crs_origin_y - lat) / hm.dy_deg.abs() * hm.dy_meters;
            (x, y)
        }
    };

    let max_x = hm.cols as f64 * hm.dx_meters;
    let max_y = hm.rows as f64 * hm.dy_meters;
    if x >= 0.0 && x <= max_x && y >= 0.0 && y <= max_y {
        Some((x as f32, y as f32))
    } else {
        None
    }
}

pub fn run(tile_path: &Path, width: u32, height: u32, vsync: bool) {
    let (scene, hm, lat_rad) = prepare_scene(tile_path, width, height);
    let dx: f32 = scene.get_dx_meters();
    let dy: f32 = scene.get_dy_meters();

    // shadow worker: capacity-1 sync channel so stale requests are never queued
    let (shadow_tx, worker_rx) = mpsc::sync_channel::<(f32, f32)>(1);
    let (worker_tx, shadow_rx) = mpsc::channel::<ShadowMask>();
    let hm_worker = Arc::clone(&hm);
    std::thread::spawn(move || {
        while let Ok((azimuth, elevation)) = worker_rx.recv() {
            let mask = terrain::compute_shadow_vector_par_with_azimuth(
                &hm_worker, azimuth, elevation, 200.0,
            );
            if worker_tx.send(mask).is_err() {
                break; // main thread dropped receiver — exit
            }
        }
    });

    let event_loop = EventLoop::new().expect("error creating winit event loop");

    // Named camera position: Hintertux glacier tongue, WGS84.
    // Converted to tile-local metres at runtime — works for any tile that contains this point.
    const CAM_LAT: f64 = 47.076211; // 47°04'34.36"N
    const CAM_LON: f64 = 11.687592; // 11°41'15.33"E
    const CAM_ELEV: f32 = 3258.0;

    let init_cam_pos = latlon_to_tile_metres(CAM_LAT, CAM_LON, &hm)
        .map(|(x, y)| [x, y, CAM_ELEV])
        .unwrap_or([2457.0 * dx, 3328.0 * dy, 3341.0]);
    let init_yaw = (19627.0_f32).atan2(1718.0_f32);
    let init_pitch = (-3472.0_f32).atan2(19702.0_f32);

    let mut viewer: Viewer = Viewer {
        scene: Some(scene),
        window: None,
        surface: None,
        surface_config: None,
        width,
        height,
        render_width: width,
        vsync,
        ao_mode: 0,
        shadows_enabled: true,
        fog_enabled: true,
        vat_mode: 2, // Mid
        lod_mode: 1, // High
        // fps counter
        fps_timer: std::time::Instant::now(),
        frame_count: 0,
        fps: 0.0,
        // initial cam pos
        last_frame: std::time::Instant::now(),
        cam_pos: init_cam_pos,
        yaw: init_yaw,
        pitch: init_pitch,
        keys_held: std::collections::HashSet::new(),
        mouse_look: false,
        immersive_mode: false,
        speed_boost: false,
        hud_renderer: None,
        hud_visible: true,
        sim_day: 172,
        sim_hour: 10.0,
        lat_rad,
        day_accum: 0.0,
        shadow_tx,
        shadow_rx,
        shadow_computing: false,
    };
    event_loop
        .run_app(&mut viewer)
        .expect("error running app from event loop in run viewer method");
}

/// Geographic solar position (Spencer 1971 declination approximation).
/// Returns (azimuth_rad, elevation_rad) where azimuth is measured clockwise from North.
fn sun_position(lat_rad: f32, day: i32, hour: f32) -> (f32, f32) {
    use std::f32::consts::TAU;
    // Solar declination
    let decl =
        23.45_f32.to_radians() * ((360.0_f32 / 365.0 * (day as f32 + 284.0)).to_radians()).sin();
    // Hour angle: 0 at solar noon, negative = morning
    let h = (15.0_f32 * (hour - 12.0)).to_radians();
    // Elevation
    let sin_el = lat_rad.sin() * decl.sin() + lat_rad.cos() * decl.cos() * h.cos();
    let elevation = sin_el.clamp(-1.0, 1.0).asin();
    // Azimuth from North, clockwise
    let cos_el = elevation.cos();
    let azimuth = if cos_el < 1e-6 {
        0.0
    } else {
        let cos_az = (decl.sin() - sin_el * lat_rad.sin()) / (cos_el * lat_rad.cos());
        let az = cos_az.clamp(-1.0, 1.0).acos();
        if h > 0.0 {
            TAU - az
        } else {
            az
        }
    };
    (azimuth, elevation)
}

fn prepare_scene(tile_path: &Path, width: u32, height: u32) -> (GpuScene, Arc<Heightmap>, f32) {
    let dem_format = tile_path
        .extension()
        .expect("error getting extension of DEM data");

    let hm: Heightmap = if dem_format == "bil" {
        match dem_io::parse_bil(tile_path) {
            Ok(hm) => hm,
            Err(error) => panic!(
                "Couldn't open the file {:?}; errors: {:?}",
                tile_path, error
            ),
        }
    } else if dem_format == "tif" {
        let scale = dem_io::geotiff_pixel_scale(tile_path);
        let result = if scale >= 5.0 {
            dem_io::parse_geotiff_epsg_31287(tile_path) // EPSG:31287 Austria Lambert (5m+)
        } else if scale >= 1.0 {
            dem_io::parse_geotiff_epsg_3035(tile_path) // EPSG:3035 LAEA Europe (1m)
        } else {
            dem_io::parse_geotiff(tile_path) // EPSG:4326 geographic (GLO-30 etc.)
        };
        match result {
            Ok(hm) => hm,
            Err(error) => panic!(
                "Couldn't open the file {:?}; errors: {:?}",
                tile_path, error
            ),
        }
    } else {
        panic!("DEM format is not supported {:?}", dem_format)
    };

    let normal_map = terrain::compute_normals_vector_par(&hm);

    // tile centre latitude — origin_lat is north edge of row 0
    let center_lat = hm.origin_lat - (hm.rows as f64 / 2.0) * hm.dy_deg.abs();
    let lat_rad = (center_lat as f32).to_radians();

    let (init_az, init_el) = sun_position(lat_rad, 172, 10.0);
    let shadow_mask: ShadowMask =
        terrain::compute_shadow_vector_par_with_azimuth(&hm, init_az, init_el, 200.0);

    // AO compute, the higher the ray_elevation_rad the less pronounced the effect (less darkening)
    let ao_data_mask: Vec<f32> =
        terrain::compute_ao_true_hemi(&hm, 16, 10.0f32.to_radians(), 200.0);

    let gpu_ctx: GpuContext = GpuContext::new();
    let hm = Arc::new(hm);
    let scene: GpuScene = GpuScene::new(
        gpu_ctx,
        &hm,
        &normal_map,
        &shadow_mask,
        &ao_data_mask,
        width,
        height,
    );

    (scene, hm, lat_rad)
}
