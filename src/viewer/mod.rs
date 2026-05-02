mod geo;
mod hud_renderer;
mod scene_init;
mod tiers;

use std::path::Path;
use std::sync::{mpsc, Arc};

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

use self::geo::{laea_epsg3035_inverse, latlon_to_tile_metres, lcc_epsg31287, sun_position};
use self::scene_init::{compute_ao_cropped, prepare_scene, INIT_SIM_DAY, INIT_SIM_HOUR};
use self::tiers::{BevBaseState, Glo30State, AO_DRIFT_THRESHOLD_M};

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
    last_shadow_az: f32,
    last_shadow_el: f32,
    // drift-based AO recompute
    ao_tx: mpsc::SyncSender<(f64, f64)>,
    ao_rx: mpsc::Receiver<Vec<f32>>,
    ao_computing: bool,
    ao_last_x: f64, // tile-local metres of last AO centre
    ao_last_y: f64,
    // base heightmap (shared with shadow worker; replaced on tile slide)
    hm: Arc<Heightmap>,
    // mode-specific sliding state (only one is Some)
    glo30: Option<Glo30State>,
    bev_base: Option<BevBaseState>,
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
        _window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => {
                // // delta time for frame-rate-independent camera movement
                let dt = self.last_frame.elapsed().as_secs_f32();
                self.last_frame = std::time::Instant::now();

                // cam movements
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
                if !self.shadow_computing && elevation > 0.0 && sun_moved {
                    if self.shadow_tx.try_send((azimuth, elevation)).is_ok() {
                        self.shadow_computing = true;
                        self.last_shadow_az = azimuth;
                        self.last_shadow_el = elevation;
                    }
                }

                // drift-based AO recompute (5 km threshold in tile-local metres)
                if let Ok(new_ao) = self.ao_rx.try_recv() {
                    self.scene.as_ref().unwrap().update_ao(&new_ao);
                    self.ao_computing = false;
                }
                if !self.ao_computing {
                    let cam_x = self.cam_pos[0] as f64;
                    let cam_y = self.cam_pos[1] as f64;
                    // recompute AO when camera drifts far enough that the 20km radius
                    // no longer fully covers the new position with good data
                    if (cam_x - self.ao_last_x).abs() > AO_DRIFT_THRESHOLD_M
                        || (cam_y - self.ao_last_y).abs() > AO_DRIFT_THRESHOLD_M
                    {
                        if self.ao_tx.try_send((cam_x, cam_y)).is_ok() {
                            self.ao_computing = true;
                            self.ao_last_x = cam_x;
                            self.ao_last_y = cam_y;
                            println!("AO recompute triggered at ({cam_x:.0}, {cam_y:.0})");
                        }
                    }
                }

                // GLO-30 tile sliding
                if let Some(ref mut glo30) = self.glo30 {
                    if let Ok(bundle) = glo30.tile_rx.try_recv() {
                        {
                            let scene = self.scene.as_mut().unwrap();
                            scene.update_heightmap(&bundle.hm, &bundle.normals, &bundle.ao);
                            scene.update_shadow(&bundle.shadow);
                            scene.set_hm5m_inactive();
                        }
                        if let Some((nx, ny)) =
                            latlon_to_tile_metres(bundle.cam_lat, bundle.cam_lon, &bundle.hm)
                        {
                            self.cam_pos[0] = nx;
                            self.cam_pos[1] = ny;
                        }
                        glo30.centre_lat = bundle.centre_lat;
                        glo30.centre_lon = bundle.centre_lon;
                        glo30.tile_loading = false;
                        self.hm = bundle.hm;
                        // respawn shadow worker with updated heightmap
                        let (new_tx, new_worker_rx) = mpsc::sync_channel::<(f32, f32)>(1);
                        let (new_worker_tx, new_rx) = mpsc::channel::<ShadowMask>();
                        let old_tx = std::mem::replace(&mut self.shadow_tx, new_tx);
                        let _ = std::mem::replace(&mut self.shadow_rx, new_rx);
                        drop(old_tx);
                        self.shadow_computing = false;
                        let hm_w = Arc::clone(&self.hm);
                        std::thread::spawn(move || {
                            while let Ok((az, el)) = new_worker_rx.recv() {
                                let mask = terrain::compute_shadow_vector_par_with_azimuth(
                                    &hm_w, az, el, 200.0,
                                );
                                if new_worker_tx.send(mask).is_err() {
                                    break;
                                }
                            }
                        });
                        println!("tile slid to N{}E{}", glo30.centre_lat, glo30.centre_lon);
                    }
                    if !glo30.tile_loading {
                        // Convert tile-local metres → absolute WGS84 lon/lat to detect
                        // which 1°×1° GLO-30 tile the camera is currently inside.
                        // X: metres / (metres/px) * (degrees/px) + left-edge longitude
                        // Y: top-edge latitude - same conversion (tile Y increases downward,
                        //    latitude increases upward, hence the subtraction)
                        let cam_lon_w = self.cam_pos[0] as f64 / self.hm.dx_meters * self.hm.dx_deg
                            + self.hm.crs_origin_x;
                        let cam_lat_w = self.hm.crs_origin_y
                            - self.cam_pos[1] as f64 / self.hm.dy_meters * self.hm.dy_deg.abs();
                        let new_lat = cam_lat_w.floor() as i32;
                        let new_lon = cam_lon_w.floor() as i32;
                        if new_lat != glo30.centre_lat || new_lon != glo30.centre_lon {
                            if glo30
                                .tile_tx
                                .try_send((new_lat, new_lon, cam_lat_w, cam_lon_w))
                                .is_ok()
                            {
                                glo30.tile_loading = true;
                                println!(
                                    "tile slide triggered: N{}E{} → N{}E{}",
                                    glo30.centre_lat, glo30.centre_lon, new_lat, new_lon
                                );
                            }
                        }
                    }
                }

                // BEV two-tier drift reload
                if let Some(ref mut bev_base) = self.bev_base {
                    // ── base tier ──
                    if let Some(data) = bev_base.base.try_recv() {
                        // re-project camera to new heightmap coords using absolute EPSG:31287 position
                        let (easting, northing) = bev_epsg_pos(self.cam_pos, &self.hm);
                        self.cam_pos[0] = (easting - data.hm.crs_origin_x) as f32;
                        self.cam_pos[1] = (data.hm.crs_origin_y - northing) as f32;
                        {
                            let scene = self.scene.as_mut().unwrap();
                            scene.update_heightmap(&data.hm, &data.normals, &data.ao);
                            scene.update_shadow(&data.shadow);
                            // The fine-tier origins are offsets relative to the base heightmap origin.
                            // After a base reload the origin shifts, so the old offsets are wrong —
                            // hide both fine tiers until their workers deliver fresh windows.
                            scene.set_hm5m_inactive();
                            scene.set_hm1m_inactive();
                        }
                        self.hm = data.hm;
                        // close and fine tier offsets were relative to the old base origin — must reload
                        bev_base.close.invalidate();
                        if let Some(ref mut fine) = bev_base.fine {
                            fine.invalidate();
                        }
                        // respawn shadow worker with updated heightmap
                        let (new_tx, new_worker_rx) = mpsc::sync_channel::<(f32, f32)>(1);
                        let (new_worker_tx, new_rx) = mpsc::channel::<ShadowMask>();
                        let old_tx = std::mem::replace(&mut self.shadow_tx, new_tx);
                        let _ = std::mem::replace(&mut self.shadow_rx, new_rx);
                        drop(old_tx);
                        self.shadow_computing = false;
                        let hm_w = Arc::clone(&self.hm);
                        std::thread::spawn(move || {
                            while let Ok((az, el)) = new_worker_rx.recv() {
                                let mask = terrain::compute_shadow_vector_par_with_azimuth(
                                    &hm_w, az, el, 200.0,
                                );
                                if new_worker_tx.send(mask).is_err() {
                                    break;
                                }
                            }
                        });
                        println!(
                            "BEV base reloaded: {}×{} at {:.1}m/px",
                            self.hm.cols, self.hm.rows, self.hm.dx_meters
                        );
                    }
                    if !bev_base.base.computing {
                        let (e, n) = bev_epsg_pos(self.cam_pos, &self.hm);
                        if bev_base.base.needs_reload(e, n) && bev_base.base.try_trigger(e, n) {
                            println!("BEV base reload triggered at ({e:.0}, {n:.0})");
                        }
                    }

                    // ── 5 m close tier ──
                    if let Some(data) = bev_base.close.try_recv() {
                        let origin_x = (data.hm.crs_origin_x - self.hm.crs_origin_x) as f32;
                        let origin_y = (self.hm.crs_origin_y - data.hm.crs_origin_y) as f32;
                        self.scene.as_mut().unwrap().upload_hm5m(
                            origin_x,
                            origin_y,
                            &data.hm,
                            &data.normals,
                            &data.shadow,
                        );
                        println!(
                            "5m tier updated: {}×{} at {:.1}m/px",
                            data.hm.cols, data.hm.rows, data.hm.dx_meters
                        );
                    }
                    if !bev_base.close.computing {
                        let (e, n) = bev_epsg_pos(self.cam_pos, &self.hm);
                        if bev_base.close.needs_reload(e, n) && bev_base.close.try_trigger(e, n) {
                            println!("5m reload triggered at ({e:.0}, {n:.0})");
                        }
                    }

                    // ── 1 m fine tier ──
                    if let Some(ref mut fine) = bev_base.fine {
                        if let Some(data) = fine.try_recv() {
                            // data.hm.crs_origin_x/y is in EPSG:3035; convert to EPSG:31287 tile-local
                            let (lat, lon) =
                                laea_epsg3035_inverse(data.hm.crs_origin_x, data.hm.crs_origin_y);
                            let (e31287, n31287) = lcc_epsg31287(lat, lon);
                            let origin_x = (e31287 - self.hm.crs_origin_x) as f32;
                            let origin_y = (self.hm.crs_origin_y - n31287) as f32;
                            self.scene.as_mut().unwrap().upload_hm1m(
                                origin_x,
                                origin_y,
                                &data.hm,
                                &data.normals,
                                &data.shadow,
                            );
                            println!(
                                "1m tier updated: {}×{} at {:.1}m/px",
                                data.hm.cols, data.hm.rows, data.hm.dx_meters
                            );
                        }
                        if !fine.computing {
                            let (e, n) = bev_epsg_pos(self.cam_pos, &self.hm);
                            if fine.needs_reload(e, n) && fine.try_trigger(e, n) {
                                println!("1m reload triggered at ({e:.0}, {n:.0})");
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

/// Convert tile-local camera position to absolute EPSG:31287 (easting, northing).
/// Used for BEV drift detection; extracted to avoid repeating the same two lines.
fn bev_epsg_pos(cam_pos: [f32; 3], hm: &Heightmap) -> (f64, f64) {
    let easting = cam_pos[0] as f64 + hm.crs_origin_x;
    let northing = hm.crs_origin_y - cam_pos[1] as f64;
    (easting, northing)
}

pub fn run(tile_path: &Path, width: u32, height: u32, vsync: bool, tiles_1m_dir: Option<&Path>) {
    // Named camera position: Hintertux glacier tongue, WGS84.
    // Converted to tile-local metres at runtime — works for any tile that contains this point.
    const CAM_LAT: f64 = 47.076211; // 47°04'34.36"N
    const CAM_LON: f64 = 11.687592; // 11°41'15.33"E
    const CAM_ELEV: f32 = 3258.0;

    let (mut scene, hm, lat_rad) = prepare_scene(tile_path, width, height, CAM_LAT, CAM_LON);
    let dx: f32 = scene.get_dx_meters();
    let dy: f32 = scene.get_dy_meters();

    let init_cam_pos = latlon_to_tile_metres(CAM_LAT, CAM_LON, &hm)
        .map(|(x, y)| [x, y, CAM_ELEV])
        .unwrap_or([2457.0 * dx, 3328.0 * dy, 3341.0]);
    let init_yaw = (19627.0_f32).atan2(1718.0_f32);
    let init_pitch = (-3472.0_f32).atan2(19702.0_f32);

    // shadow worker
    let (shadow_tx, worker_rx) = mpsc::sync_channel::<(f32, f32)>(1);
    let (worker_tx, shadow_rx) = mpsc::channel::<ShadowMask>();
    {
        let hm_w = Arc::clone(&hm);
        std::thread::spawn(move || {
            while let Ok((az, el)) = worker_rx.recv() {
                let mask = terrain::compute_shadow_vector_par_with_azimuth(&hm_w, az, el, 200.0);
                if worker_tx.send(mask).is_err() {
                    break;
                }
            }
        });
    }

    // AO worker
    let (ao_tx, ao_worker_rx) = mpsc::sync_channel::<(f64, f64)>(1);
    let (ao_worker_tx, ao_rx) = mpsc::channel::<Vec<f32>>();
    {
        let hm_ao = Arc::clone(&hm);
        std::thread::spawn(move || {
            while let Ok((cam_x, cam_y)) = ao_worker_rx.recv() {
                let ao = compute_ao_cropped(&hm_ao, cam_x, cam_y);
                if ao_worker_tx.send(ao).is_err() {
                    break;
                }
            }
        });
    }

    // Mode-specific state setup
    let glo30: Option<Glo30State>;
    let bev_base: Option<BevBaseState>;

    if hm.crs_epsg == 31287 {
        // BEV COG mode: three resolution tiers (base IFD-2/1, close 5m IFD-0, fine 1m tiles).
        // IFD levels for this file:
        //   IFD-0 = 5 m/px  — full resolution, always present by TIFF spec
        //   IFD-1 ≈ 10 m/px — first overview (optional)
        //   IFD-2 ≈ 20 m/px — second overview (optional)
        // We render two tiers: a wide base at BEV_BASE_IFD (falls back to IFD-1 if absent)
        // and a close 5 m window at IFD-0.  IFD-0 is NOT viable for the 60 km base window
        // (~12 000×12 000 px at 5 m/px, exceeds the wgpu 8 192-pixel texture limit).
        glo30 = None;
        let (init_e, init_n) = lcc_epsg31287(CAM_LAT, CAM_LON);
        bev_base = Some(BevBaseState::new(
            tile_path,
            lat_rad,
            init_e,
            init_n,
            &hm,
            tiles_1m_dir,
            &mut scene,
        ));
    } else if hm.crs_epsg == 4326 {
        // GLO-30 mode: sliding 3×3 Copernicus tile grid.
        bev_base = None;
        let tiles_dir = tile_path
            .parent()
            .and_then(|p| p.parent())
            .unwrap_or(Path::new("tiles"))
            .to_path_buf();
        glo30 = Some(Glo30State::new(&tiles_dir, lat_rad, CAM_LAT, CAM_LON));
    } else {
        // EPSG:3035 single tile (Hintertux-style) — static, no sliding
        glo30 = None;
        bev_base = None;
    }

    let event_loop = EventLoop::new().expect("error creating winit event loop");

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
        hm,
        glo30,
        bev_base,
    };
    event_loop
        .run_app(&mut viewer)
        .expect("error running app from event loop in run viewer method");
}
