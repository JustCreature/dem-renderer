use std::{path::Path, sync::Arc};

use dem_io::Heightmap;
use render_gpu::{GpuContext, GpuScene};
use terrain::ShadowMask;
use wgpu::{Adapter, Buffer, Device, Instance, Surface};
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::WindowEvent,
    event_loop::EventLoop,
    window::{Window, WindowAttributes},
};

struct Viewer {
    scene: Option<GpuScene>,
    window: Option<Arc<Window>>,
    surface: Option<wgpu::Surface<'static>>,
    surface_config: Option<wgpu::SurfaceConfiguration>,
    width: u32,
    height: u32,
    // fps counter
    last_frame: std::time::Instant,
    frame_count: u32,
    fps: f64,
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

        let instance: &Instance = &scene.get_gpu_ctx().instance;

        self.surface = Some(
            instance
                .create_surface(window.clone())
                .expect("error creating a surface from default Instance in resumed method"),
        );

        // surface configuration
        let ctx: &GpuContext = scene.get_gpu_ctx();
        let adapter: &Adapter = &ctx.adapter;
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

        let config: wgpu::SurfaceConfiguration = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::COPY_DST,
            format,
            width: self.width,
            height: self.height,
            present_mode: wgpu::PresentMode::Immediate,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        let device: &Device = &ctx.device;
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
                let surface: &Surface = self.surface.as_ref().expect("no surface for window event");
                let scene: &GpuScene = self.scene.as_ref().expect("no scene for window event");
                let ctx: &GpuContext = scene.get_gpu_ctx();
                let surface_texture = match surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(t) => t,
                    wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
                    _ => return, // Timeout or occluded — skip this frame
                };

                // Camera above the terrain, looking at Olperer
                // pixel space: origin at (col=2388, row=3341), z = terrain_height + 1800m
                // look_at: Olperer at (col=2527, row=3467), z = 3476m
                // TODO: remove block this later
                let dx: f32 = scene.get_dx_meters();
                let dy: f32 = scene.get_dy_meters();

                // benchmark with google earth
                let cam_col = 2457.0f32;
                let cam_row = 3328.0f32;

                let sun_dir = [0.4f32, 0.5f32, 0.7f32]; // [east, south, up] — morning sun NE
                let sun_azimuth_rad = (sun_dir[0]).atan2(-sun_dir[1]); // atan2(east, north)
                let sun_elevation_rad =
                    sun_dir[2].atan2((sun_dir[0].powi(2) + sun_dir[1].powi(2)).sqrt());

                let mut encoder =
                    ctx.device
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("blit_enc"),
                        });

                scene.dispatch_frame(
                    &mut encoder,
                    [cam_col * dx, cam_row * dy, 3341.0],
                    [cam_col * dx + 19_627.0, cam_row * dy - 1_718.0, -131.0],
                    70.0,
                    self.width as f32 / self.height as f32,
                    sun_dir,
                    scene.get_dx_meters() / 0.8,
                    200_000.0,
                );
                let output_buf: &Buffer = scene.get_output_buffer();

                encoder.copy_buffer_to_texture(
                    wgpu::TexelCopyBufferInfo {
                        buffer: &output_buf,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(self.width * 4), // 4 bytes per RGBA pixel
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
                ctx.queue.submit([encoder.finish()]);
                surface_texture.present();

                // fps counter
                self.frame_count += 1;
                let elapsed = self.last_frame.elapsed().as_secs_f64();
                if elapsed >= 1.0 {
                    self.fps = self.frame_count as f64 / elapsed;
                    self.last_frame = std::time::Instant::now();
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
            _ => {}
        }
    }
}

pub fn run(tile_path: &Path, width: u32, height: u32) {
    let scene: GpuScene = prepare_scene(tile_path, width, height);

    let event_loop = EventLoop::new().expect("error creating winit event loop");
    let mut viewer: Viewer = Viewer {
        scene: Some(scene),
        window: None,
        surface: None,
        surface_config: None,
        width,
        height,
        last_frame: std::time::Instant::now(),
        frame_count: 0,
        fps: 0.0,
    };
    event_loop
        .run_app(&mut viewer)
        .expect("error running app from event loop in run viewer method");
}

fn prepare_scene(tile_path: &Path, width: u32, height: u32) -> GpuScene {
    let hm: Heightmap = match dem_io::parse_bil(tile_path) {
        Ok(hm) => hm,
        Err(error) => panic!(
            "Couldn't open the file {:?}; errors: {:?}",
            tile_path, error
        ),
    };

    // let normals_map: NormalMap = terrain::compute_normals_vector_par(&hm);

    const SUN_DIR: [f32; 3] = [0.4, 0.5, 0.7];
    let sun_azimuth_rad = (SUN_DIR[0]).atan2(-SUN_DIR[1]);
    let sun_elevation_rad = SUN_DIR[2].atan2((SUN_DIR[0].powi(2) + SUN_DIR[1].powi(2)).sqrt());
    let shadow_mask: ShadowMask =
        terrain::compute_shadow_vector_par_with_azimuth(&hm, sun_azimuth_rad, sun_elevation_rad);

    let gpu_ctx: GpuContext = GpuContext::new();
    let scene: GpuScene = GpuScene::new(gpu_ctx, &hm, &shadow_mask, width, height);

    scene
}
