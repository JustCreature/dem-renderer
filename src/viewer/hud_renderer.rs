struct HudBackground {
    pipeline: wgpu::RenderPipeline,
    vertex_buf: wgpu::Buffer,
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

pub struct HudRenderer {
    font_system: glyphon::FontSystem,
    swash_cache: glyphon::SwashCache,
    text_atlas: glyphon::TextAtlas,
    text_renderer: glyphon::TextRenderer,
    fps_buffer: glyphon::Buffer,
    hint_buffer: glyphon::Buffer,
    viewport: glyphon::Viewport,
    hud_bg: HudBackground,
    width: u32,
    height: u32,
}

impl HudBackground {
    pub fn new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("shader_hud_bg.wgsl"));

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uniform_buf"),
            size: 8,
            mapped_at_creation: false,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hud_bg_init_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vertex_buf"),
            size: 96,
            mapped_at_creation: false,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hud_bg_bg"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });

        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hud_bg_init_pl"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hud_bg"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 8,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x2,
                        offset: 0,
                        shader_location: 0,
                    }],
                }],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::SrcAlpha,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent::OVER,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        HudBackground {
            pipeline,
            vertex_buf,
            uniform_buf,
            bind_group,
        }
    }

    pub fn update_size(&self, queue: &wgpu::Queue, width: u32, height: u32) {
        let vetices = build_vertices(width, height);
        queue.write_buffer(&self.vertex_buf, 0, bytemuck::cast_slice(&vetices));
        queue.write_buffer(
            &self.uniform_buf,
            0,
            bytemuck::cast_slice(&[width as f32, height as f32]),
        );
    }

    pub fn draw<'a>(&'a self, rpass: &mut wgpu::RenderPass<'a>) {
        rpass.set_pipeline(&self.pipeline);
        rpass.set_bind_group(0, &self.bind_group, &[]);
        rpass.set_vertex_buffer(0, self.vertex_buf.slice(..));
        rpass.draw(0..12, 0..1);
    }
}

fn build_vertices(width: u32, height: u32) -> [f32; 24] {
    [
        //For the fps box (x0=4, y0=4, x1=180, y1=36):
        // triangle 1
        4.0,
        4.0, // (x0, y0)
        180.0,
        4.0, // (x1, y0)
        180.0,
        36.0, // (x1, y1)
        // triangle 2
        4.0,
        4.0, // (x0, y0)
        180.0,
        36.0, // (x1, y1)
        4.0,
        36.0, // (x0, y1)
        // For the hint box (x0=0, y0=height-36, x1=width, y1=height-4):
        // triangle 1
        0.0,
        height as f32 - 36.0, // (x0, y0)
        width as f32,
        height as f32 - 36.0, // (x1, y0)
        width as f32,
        height as f32 - 4.0, // (x1, y1)
        // triangle 2
        0.0,
        height as f32 - 36.0, // (x0, y0)
        width as f32,
        height as f32 - 4.0, // (x1, y1)
        0.0,
        height as f32 - 4.0, // (x0, y1)
    ]
}

impl HudRenderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> Self {
        let mut font_system: glyphon::FontSystem = glyphon::FontSystem::new();
        let mut swash_cache: glyphon::SwashCache = glyphon::SwashCache::new();
        let mut cache: glyphon::Cache = glyphon::Cache::new(device);
        let mut text_atlas: glyphon::TextAtlas =
            glyphon::TextAtlas::new(device, queue, &cache, format);
        let text_renderer: glyphon::TextRenderer = glyphon::TextRenderer::new(
            &mut text_atlas,
            device,
            wgpu::MultisampleState::default(),
            None,
        );
        let mut fps_buffer: glyphon::Buffer =
            glyphon::Buffer::new(&mut font_system, glyphon::Metrics::new(18.0, 20.0));
        fps_buffer.set_size(&mut font_system, Some(400.0), Some(40.0));
        fps_buffer.set_text(
            &mut font_system,
            "0 fps",
            &glyphon::Attrs::new(),
            glyphon::Shaping::Basic,
            None,
        );
        let mut hint_buffer: glyphon::Buffer =
            glyphon::Buffer::new(&mut font_system, glyphon::Metrics::new(18.0, 20.0));
        hint_buffer.set_size(&mut font_system, Some(width as f32), Some(40.0));
        hint_buffer.set_text(
            &mut font_system,
            "Q - activate/deactivate immersive mode; E - hide/show HUD",
            &glyphon::Attrs::new(),
            glyphon::Shaping::Basic,
            Some(glyphon::cosmic_text::Align::Center),
        );
        let hud_bg: HudBackground = HudBackground::new(device, width, height, format);
        let viewport: glyphon::Viewport = glyphon::Viewport::new(device, &cache);

        HudRenderer {
            font_system,
            swash_cache,
            text_atlas,
            text_renderer,
            fps_buffer,
            hint_buffer,
            viewport,
            hud_bg,
            width,
            height,
        }
    }

    pub fn update_size(&mut self, queue: &wgpu::Queue, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.hint_buffer
            .set_size(&mut self.font_system, Some(width as f32), Some(40.0));
        self.hud_bg.update_size(queue, width, height);
    }

    pub fn draw(
        &mut self,
        queue: &wgpu::Queue,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        surface_view: &wgpu::TextureView,
        fps: f32,
        ms: f32,
    ) {
        self.fps_buffer.set_text(
            &mut self.font_system,
            &format!("{:.0} fps  {:.1} ms", fps, ms / fps.max(0.001)),
            &glyphon::Attrs::new(),
            glyphon::Shaping::Basic,
            None,
        );
        self.viewport.update(
            queue,
            glyphon::Resolution {
                width: self.width,
                height: self.height,
            },
        );
        self.text_renderer
            .prepare(
                device,
                queue,
                &mut self.font_system,
                &mut self.text_atlas,
                &self.viewport,
                [
                    glyphon::TextArea {
                        buffer: &self.fps_buffer,
                        left: 10.0,
                        top: 10.0,
                        scale: 1.0,
                        bounds: glyphon::TextBounds {
                            left: 0,
                            top: 0,
                            right: self.width as i32,
                            bottom: self.height as i32,
                        },
                        default_color: glyphon::Color::rgb(255, 255, 255),
                        custom_glyphs: &[],
                    },
                    glyphon::TextArea {
                        buffer: &self.hint_buffer,
                        left: 0.0,
                        top: self.height as f32 - 30.0,
                        scale: 1.0,
                        bounds: glyphon::TextBounds {
                            left: 0,
                            top: 0,
                            right: self.width as i32,
                            bottom: self.height as i32,
                        },
                        default_color: glyphon::Color::rgb(255, 255, 255),
                        custom_glyphs: &[],
                    },
                ],
                &mut self.swash_cache,
            )
            .unwrap();

        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("hud"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &surface_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        self.hud_bg.draw(&mut rpass);
        self.text_renderer
            .render(&self.text_atlas, &self.viewport, &mut rpass)
            .unwrap();
        // begin_render_pass borrows encoder mutably, which means you can't use encoder again until the pass is dropped.
        // The explicit drop(rpass) releases that borrow before encoder.finish().
        drop(rpass);
    }
}
