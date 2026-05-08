use std::sync::Arc;

use egui::{
    ColorImage, FontData, FontDefinitions, FontFamily, TextureHandle, TextureOptions, ViewportId,
};
use egui_wgpu::ScreenDescriptor;
use winit::window::Window;

pub struct EguiRenderer {
    pub ctx: egui::Context,
    pub state: egui_winit::State,
    pub renderer: egui_wgpu::Renderer,
    pub bg_texture: TextureHandle,
    pub bg_w: f32,
    pub bg_h: f32,
    surface: wgpu::Surface<'static>,
    pub surface_config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

impl EguiRenderer {
    pub fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        surface: wgpu::Surface<'static>,
        surface_format: wgpu::TextureFormat,
        surface_config: wgpu::SurfaceConfiguration,
        window: &Window,
    ) -> Self {
        let ctx = egui::Context::default();
        ctx.set_fonts(load_fonts());
        apply_style(&ctx);

        // Query GPU limit first — needed both for egui's font atlas and the bg texture.
        let max_tex_side = device.limits().max_texture_dimension_2d;

        let viewport_id = ViewportId::ROOT;
        let state = egui_winit::State::new(
            ctx.clone(),
            viewport_id,
            window,
            None,
            None,
            Some(max_tex_side as usize),
        );

        let renderer = egui_wgpu::Renderer::new(
            &device,
            surface_format,
            egui_wgpu::RendererOptions::default(),
        );

        // Load background image — respect the GPU's max texture dimension
        let (bg_texture, bg_w, bg_h) = load_bg_texture(&ctx, max_tex_side);

        EguiRenderer {
            ctx,
            state,
            renderer,
            bg_texture,
            bg_w,
            bg_h,
            surface,
            surface_config,
            device,
            queue,
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            // When a window is minimized on some platforms (Windows in particular), winit fires a Resized event with dimensions 0×0.
            // If you pass those to surface.configure(), wgpu panics — a zero-size surface is invalid.
            // The guard just skips the reconfigure in that case and leaves the existing surface config
            // untouched until the window is restored to a real size.
            return;
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(&self.device, &self.surface_config);
    }

    pub fn render(&mut self, window: &Window, mut draw_ui: impl FnMut(&egui::Context)) {
        let raw_input = self.state.take_egui_input(window);

        let full_output = self.ctx.run_ui(raw_input, |ui| draw_ui(ui.ctx()));

        self.state
            .handle_platform_output(window, full_output.platform_output);

        let pixels_per_point = self.ctx.pixels_per_point();
        let screen_desc = ScreenDescriptor {
            size_in_pixels: [self.surface_config.width, self.surface_config.height],
            pixels_per_point,
        };

        let tris = self.ctx.tessellate(full_output.shapes, pixels_per_point);

        // Upload textures unconditionally — must happen before surface acquisition
        // so that textures are registered even if a frame is skipped (surface not ready).
        for (id, delta) in &full_output.textures_delta.set {
            self.renderer
                .update_texture(&self.device, &self.queue, *id, delta);
        }
        for id in &full_output.textures_delta.free {
            self.renderer.free_texture(id);
        }

        let surface_texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => t,
            wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            _ => return,
        };
        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("egui_encoder"),
            });

        self.renderer
            .update_buffers(&self.device, &self.queue, &mut encoder, &tris, &screen_desc);

        {
            let mut rpass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("egui_rpass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.02,
                                g: 0.02,
                                b: 0.02,
                                a: 1.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                })
                .forget_lifetime();
            self.renderer.render(&mut rpass, &tris, &screen_desc);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        surface_texture.present();
    }

    pub fn take_surface(self) -> wgpu::Surface<'static> {
        self.surface
    }
}

fn load_bg_texture(ctx: &egui::Context, max_side: u32) -> (TextureHandle, f32, f32) {
    let img = image::load_from_memory(include_bytes!("../../assets/mountain-bg.png"))
        .expect("embedded mountain-bg.png is invalid")
        .into_rgba8();
    let (w, h) = img.dimensions();

    // Downscale if either dimension exceeds the GPU's max texture size (e.g. 2048 on Intel).
    let img = if w > max_side || h > max_side {
        let scale = (max_side as f32 / w.max(h) as f32).min(1.0);
        let nw = ((w as f32 * scale) as u32).max(1);
        let nh = ((h as f32 * scale) as u32).max(1);
        image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let (w, h) = img.dimensions();

    let pixels: Vec<egui::Color32> = img
        .pixels()
        .map(|p| egui::Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3]))
        .collect();
    let color_image = ColorImage {
        size: [w as usize, h as usize],
        source_size: egui::Vec2::new(w as f32, h as f32),
        pixels,
    };
    let handle = ctx.load_texture("mountain-bg", color_image, TextureOptions::LINEAR);
    (handle, w as f32, h as f32)
}

fn load_fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();

    fonts.font_data.insert(
        "SpaceGrotesk".to_owned(),
        Arc::new(FontData::from_static(include_bytes!(
            "../../assets/fonts/SpaceGrotesk-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        "JetBrainsMono".to_owned(),
        Arc::new(FontData::from_static(include_bytes!(
            "../../assets/fonts/JetBrainsMono-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        "JetBrainsMonoLight".to_owned(),
        Arc::new(FontData::from_static(include_bytes!(
            "../../assets/fonts/JetBrainsMono-Light.ttf"
        ))),
    );

    fonts
        .families
        .insert(FontFamily::Proportional, vec!["SpaceGrotesk".to_owned()]);
    fonts
        .families
        .insert(FontFamily::Monospace, vec!["JetBrainsMonoLight".to_owned()]);
    fonts.families.insert(
        FontFamily::Name("SpaceGrotesk-Medium".into()),
        vec!["SpaceGrotesk".to_owned()],
    );

    fonts
}

fn apply_style(ctx: &egui::Context) {
    let mut style = (*ctx.global_style()).clone();
    style.visuals.window_fill = egui::Color32::TRANSPARENT;
    style.visuals.panel_fill = egui::Color32::TRANSPARENT;
    style.visuals.override_text_color = Some(egui::Color32::from_rgb(232, 228, 220));
    style.visuals.widgets.noninteractive.bg_stroke = egui::Stroke::NONE;
    style.visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
    style.visuals.widgets.active.bg_stroke = egui::Stroke::NONE;
    style.visuals.widgets.hovered.bg_stroke = egui::Stroke::NONE;
    // remove egui's default rounding, shadows, etc. that clash with custom drawing
    style.visuals.window_shadow = egui::Shadow::NONE;
    style.visuals.popup_shadow = egui::Shadow::NONE;
    style.spacing.item_spacing = egui::vec2(4.0, 2.0);
    // Without this, dragging over any ui.label() produces a blue text-selection highlight.
    style.interaction.selectable_labels = false;
    ctx.set_global_style(style);
}
