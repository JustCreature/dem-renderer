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
    settings_buffer: glyphon::Buffer,
    // Cardinal labels around season circle (static)
    lbl_summer: glyphon::Buffer,
    lbl_fall: glyphon::Buffer,
    lbl_winter: glyphon::Buffer,
    lbl_spring: glyphon::Buffer,
    // Cardinal labels around time circle (static)
    lbl_12: glyphon::Buffer,
    lbl_15: glyphon::Buffer,
    lbl_18: glyphon::Buffer,
    lbl_21: glyphon::Buffer,
    // Current-value labels (updated every frame)
    season_current_buf: glyphon::Buffer,
    time_current_buf: glyphon::Buffer,
    viewport: glyphon::Viewport,
    hud_bg: HudBackground,
    sun_indicator: SunIndicator,
    width: u32,
    height: u32,
    sim_day: i32,
    sim_hour: f32,
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
            size: 144,
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
        rpass.draw(0..18, 0..1);
    }
}

fn build_vertices(width: u32, height: u32) -> [f32; 36] {
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
        //For the settings box (x0=width-296, y0=4, x1=width-4, y1=36):
        // triangle 1
        width as f32 - 296.0,
        4.0, // (x0, y0)
        width as f32 - 4.0,
        4.0, // (x1, y0)
        width as f32 - 4.0,
        36.0, // (x1, y1)
        // triangle 2
        width as f32 - 296.0,
        4.0,
        width as f32 - 4.0,
        36.0,
        width as f32 - 296.0,
        36.0,
    ]
}

// ── Sun / season indicator ────────────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SunHudUniform {
    screen_w: f32,
    screen_h: f32,
    cx1: f32,
    cy1: f32,
    cx2: f32,
    cy2: f32,
    radius: f32,
    day_angle: f32,
    hour_angle: f32,
    _pad: [f32; 3],
}

struct SunIndicator {
    pipeline: wgpu::RenderPipeline,
    vertex_buf: wgpu::Buffer,
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    // cached pixel positions so HudRenderer can place text labels beside the circles
    cx: f32,
    cy1: f32, // season circle centre Y
    cy2: f32, // time circle centre Y
}

impl SunIndicator {
    const RADIUS: f32 = 50.0;
    const RIGHT_MARGIN: f32 = 80.0; // from right edge (room for "Fall"/"15:00" labels)
    const BOTTOM_OFFSET: f32 = 118.0; // hint_bar(36)+label_h(18)+padding(14)+radius(50)
    const GAP: f32 = 60.0; // between circles

    fn new(device: &wgpu::Device, width: u32, height: u32, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("shader_sun_hud.wgsl"));
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sun_hud_uniform"),
            size: std::mem::size_of::<SunHudUniform>() as u64,
            mapped_at_creation: false,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // full-screen NDC quad (6 vertices, static — never updated)
        let ndc_quad: [f32; 12] = [
            -1.0, -1.0, 1.0, -1.0, 1.0, 1.0, -1.0, -1.0, 1.0, 1.0, -1.0, 1.0,
        ];
        let vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sun_hud_vb"),
            size: (ndc_quad.len() * 4) as u64,
            mapped_at_creation: false,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sun_hud_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sun_hud_bg"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });

        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sun_hud_pl"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sun_hud_pipeline"),
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

        let mut indicator = SunIndicator {
            pipeline,
            vertex_buf,
            uniform_buf,
            bind_group,
            cx: 0.0,
            cy1: 0.0,
            cy2: 0.0,
        };
        // cx/cy1/cy2 are set correctly on the first update() call (requires queue).
        indicator
    }

    fn update(
        &mut self,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        sim_day: i32,
        sim_hour: f32,
    ) {
        use std::f32::consts::TAU;
        // Summer (day 172) = top of season circle; 12:00 = top of time circle.
        let day_angle = (sim_day as f32 - 172.0).rem_euclid(365.0) / 365.0 * TAU;
        let hour_angle = (sim_hour % 12.0) / 12.0 * TAU;

        // Right side, stacked vertically: season (top), time (bottom).
        let cx = width as f32 - Self::RIGHT_MARGIN - Self::RADIUS;
        let cy2 = height as f32 - Self::BOTTOM_OFFSET;
        let cy1 = cy2 - 2.0 * Self::RADIUS - Self::GAP;
        self.cx = cx;
        self.cy1 = cy1;
        self.cy2 = cy2;

        let u = SunHudUniform {
            screen_w: width as f32,
            screen_h: height as f32,
            cx1: cx,
            cy1,
            cx2: cx,
            cy2,
            radius: Self::RADIUS,
            day_angle,
            hour_angle,
            _pad: [0.0; 3],
        };
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&u));

        // write static NDC quad vertices (idempotent — same data every call)
        let ndc_quad: [f32; 12] = [
            -1.0, -1.0, 1.0, -1.0, 1.0, 1.0, -1.0, -1.0, 1.0, 1.0, -1.0, 1.0,
        ];
        queue.write_buffer(&self.vertex_buf, 0, bytemuck::cast_slice(&ndc_quad));
    }

    fn draw<'a>(&'a self, rpass: &mut wgpu::RenderPass<'a>) {
        rpass.set_pipeline(&self.pipeline);
        rpass.set_bind_group(0, &self.bind_group, &[]);
        rpass.set_vertex_buffer(0, self.vertex_buf.slice(..));
        rpass.draw(0..6, 0..1);
    }
}

fn day_to_date(day: i32) -> String {
    const MONTHS: [(&str, i32); 12] = [
        ("Jan", 31),
        ("Feb", 28),
        ("Mar", 31),
        ("Apr", 30),
        ("May", 31),
        ("Jun", 30),
        ("Jul", 31),
        ("Aug", 31),
        ("Sep", 30),
        ("Oct", 31),
        ("Nov", 30),
        ("Dec", 31),
    ];
    let mut rem = day.clamp(1, 365);
    for (name, days) in &MONTHS {
        if rem <= *days {
            return format!("{} {}", name, rem);
        }
        rem -= days;
    }
    // It's a fallback for the case when the loop exhausts all 12 months without returning.
    // In theory it should never be reached because day.clamp(1, 365) guarantees
    // the input is at most 365.
    // It exists purely to satisfy the Rust compiler.
    "Dec 31".to_string()
}

fn make_small_label(font_system: &mut glyphon::FontSystem, text: &str) -> glyphon::Buffer {
    let mut buf = glyphon::Buffer::new(font_system, glyphon::Metrics::new(13.0, 16.0));
    buf.set_size(font_system, Some(60.0), Some(20.0));
    buf.set_text(
        font_system,
        text,
        &glyphon::Attrs::new(),
        glyphon::Shaping::Basic,
        Some(glyphon::cosmic_text::Align::Center),
    );
    buf
}

fn make_current_label(font_system: &mut glyphon::FontSystem, text: &str) -> glyphon::Buffer {
    let mut buf = glyphon::Buffer::new(font_system, glyphon::Metrics::new(13.0, 16.0));
    buf.set_size(font_system, Some(90.0), Some(20.0));
    buf.set_text(
        font_system,
        text,
        &glyphon::Attrs::new(),
        glyphon::Shaping::Basic,
        Some(glyphon::cosmic_text::Align::Right),
    );
    buf
}

// Build a TextArea with default clipping bounds (full screen).
fn build_label_text_area(
    buffer: &glyphon::Buffer,
    left: f32,
    top: f32,
    width: u32,
    height: u32,
    color: glyphon::Color,
) -> glyphon::TextArea<'_> {
    glyphon::TextArea {
        buffer,
        left,
        top,
        scale: 1.0,
        bounds: glyphon::TextBounds {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        },
        default_color: color,
        custom_glyphs: &[],
    }
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
            "Q - activate/deactivate immersive mode; Left Cmd/Alt - speed boost; E - hide/show HUD; - or + - rewind time backward or forward; [ or ] - rewind days backward or forward",
            &glyphon::Attrs::new(),
            glyphon::Shaping::Basic,
            Some(glyphon::cosmic_text::Align::Center),
        );
        let mut settings_buffer: glyphon::Buffer =
            glyphon::Buffer::new(&mut font_system, glyphon::Metrics::new(18.0, 20.0));
        settings_buffer.set_size(&mut font_system, Some(292.0), Some(40.0));
        settings_buffer.set_text(
            &mut font_system,
            "AO: Off             (Press / to change)",
            &glyphon::Attrs::new(),
            glyphon::Shaping::Basic,
            Some(glyphon::cosmic_text::Align::Right),
        );
        // Static cardinal labels
        let lbl_summer = make_small_label(&mut font_system, "Summer");
        let lbl_fall = make_small_label(&mut font_system, "Fall");
        let lbl_winter = make_small_label(&mut font_system, "Winter");
        let lbl_spring = make_small_label(&mut font_system, "Spring");
        let lbl_12 = make_small_label(&mut font_system, "12:00");
        let lbl_15 = make_small_label(&mut font_system, "15:00");
        let lbl_18 = make_small_label(&mut font_system, "18:00");
        let lbl_21 = make_small_label(&mut font_system, "21:00");
        // Dynamic current-value labels
        let season_current_buf = make_current_label(&mut font_system, "Day 172");
        let time_current_buf = make_current_label(&mut font_system, "10:00");
        let hud_bg: HudBackground = HudBackground::new(device, width, height, format);
        let mut sun_indicator = SunIndicator::new(device, width, height, format);
        // Write initial vertex + uniform data (requires queue, done here after construction)
        sun_indicator.update(queue, width, height, 172, 10.0);
        let viewport: glyphon::Viewport = glyphon::Viewport::new(device, &cache);

        HudRenderer {
            font_system,
            swash_cache,
            text_atlas,
            text_renderer,
            fps_buffer,
            hint_buffer,
            settings_buffer,
            lbl_summer,
            lbl_fall,
            lbl_winter,
            lbl_spring,
            lbl_12,
            lbl_15,
            lbl_18,
            lbl_21,
            season_current_buf,
            time_current_buf,
            viewport,
            hud_bg,
            sun_indicator,
            width,
            height,
            sim_day: 172,
            sim_hour: 10.0,
        }
    }

    pub fn update_size(&mut self, queue: &wgpu::Queue, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.hint_buffer
            .set_size(&mut self.font_system, Some(width as f32), Some(40.0));
        self.hud_bg.update_size(queue, width, height);
        self.sun_indicator
            .update(queue, width, height, self.sim_day, self.sim_hour);
    }

    pub fn draw(
        &mut self,
        queue: &wgpu::Queue,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        surface_view: &wgpu::TextureView,
        fps: f32,
        ms: f32,
        sim_day: i32,
        sim_hour: f32,
        ao_mode: u32,
    ) {
        self.sim_day = sim_day;
        self.sim_hour = sim_hour;
        self.sun_indicator
            .update(queue, self.width, self.height, sim_day, sim_hour);

        // Update current-value labels
        let day_text = day_to_date(sim_day);
        self.season_current_buf.set_text(
            &mut self.font_system,
            &day_text,
            &glyphon::Attrs::new(),
            glyphon::Shaping::Basic,
            Some(glyphon::cosmic_text::Align::Right),
        );
        let h = sim_hour as u32 % 24;
        let m = ((sim_hour.fract()) * 60.0) as u32;
        let time_text = format!("Time: {:02}:{:02}", h, m);
        self.time_current_buf.set_text(
            &mut self.font_system,
            &time_text,
            &glyphon::Attrs::new(),
            glyphon::Shaping::Basic,
            Some(glyphon::cosmic_text::Align::Right),
        );

        self.fps_buffer.set_text(
            &mut self.font_system,
            &format!("{:.0} fps  {:.1} ms", fps, ms / fps.max(0.001)),
            &glyphon::Attrs::new(),
            glyphon::Shaping::Basic,
            None,
        );
        let ao_label = match ao_mode {
            0 => "AO: Off          (Press / to change)",
            1 => "AO: SSAO ×8      (Press / to change)",
            2 => "AO: SSAO ×16     (Press / to change)",
            3 => "AO: HBAO ×4      (Press / to change)",
            4 => "AO: HBAO ×8      (Press / to change)",
            _ => "AO: True Hemi    (Press / to change)",
        };
        self.settings_buffer.set_text(
            &mut self.font_system,
            ao_label,
            &glyphon::Attrs::new(),
            glyphon::Shaping::Basic,
            Some(glyphon::cosmic_text::Align::Right),
        );
        self.viewport.update(
            queue,
            glyphon::Resolution {
                width: self.width,
                height: self.height,
            },
        );
        // Compute label positions from stored circle centres.
        let cx = self.sun_indicator.cx;
        let cy1 = self.sun_indicator.cy1;
        let cy2 = self.sun_indicator.cy2;
        let r = SunIndicator::RADIUS;
        let lw = 60.0_f32; // label box width for cardinal labels
        let dim = glyphon::Color::rgb(210, 210, 210);
        let shd = glyphon::Color::rgba(0, 0, 0, 160); // drop-shadow colour (semi-transparent black)
        let w = self.width;
        let h = self.height;

        // Current-value labels: right-aligned 90px box at 10–11 o'clock, outside ring.
        let cur_w = 90.0_f32;
        let cur_l = cx - r - cur_w - 4.0;
        let sc_t = cy1 - r * 0.5 - 8.0; // season current top
        let tc_t = cy2 - r * 0.5 - 8.0; // time current top

        // Each cardinal label and current-value label is rendered twice:
        //   1. Shadow pass — same buffer, offset (+1, +1), dark semi-transparent colour.
        //   2. Real pass  — normal position, light colour.
        // glyphon composites TextAreas in order, so shadows land under the real glyphs.
        self.text_renderer
            .prepare(
                device,
                queue,
                &mut self.font_system,
                &mut self.text_atlas,
                &self.viewport,
                [
                    // FPS counter (top-left)
                    glyphon::TextArea {
                        buffer: &self.fps_buffer,
                        left: 10.0,
                        top: 10.0,
                        scale: 1.0,
                        bounds: glyphon::TextBounds {
                            left: 0,
                            top: 0,
                            right: w as i32,
                            bottom: h as i32,
                        },
                        default_color: glyphon::Color::rgb(255, 255, 255),
                        custom_glyphs: &[],
                    },
                    //Settings
                    glyphon::TextArea {
                        buffer: &self.settings_buffer,
                        left: w as f32 - 298.0,
                        top: 10.0,
                        scale: 1.0,
                        bounds: glyphon::TextBounds {
                            left: 0,
                            top: 0,
                            right: w as i32,
                            bottom: h as i32,
                        },
                        default_color: glyphon::Color::rgb(255, 255, 255),
                        custom_glyphs: &[],
                    },
                    // Hint bar (bottom)
                    glyphon::TextArea {
                        buffer: &self.hint_buffer,
                        left: 0.0,
                        top: h as f32 - 30.0,
                        scale: 1.0,
                        bounds: glyphon::TextBounds {
                            left: 0,
                            top: 0,
                            right: w as i32,
                            bottom: h as i32,
                        },
                        default_color: glyphon::Color::rgb(255, 255, 255),
                        custom_glyphs: &[],
                    },
                    // ── Season circle labels — shadows ───────────────────────
                    build_label_text_area(
                        &self.lbl_summer,
                        cx - lw / 2.0 + 1.0,
                        cy1 - r - 19.0,
                        w,
                        h,
                        shd,
                    ),
                    build_label_text_area(
                        &self.lbl_winter,
                        cx - lw / 2.0 + 1.0,
                        cy1 + r + 5.0,
                        w,
                        h,
                        shd,
                    ),
                    build_label_text_area(&self.lbl_fall, cx + r + 5.0, cy1 - 7.0, w, h, shd),
                    build_label_text_area(
                        &self.lbl_spring,
                        cx - r - lw - 3.0,
                        cy1 - 7.0,
                        w,
                        h,
                        shd,
                    ),
                    build_label_text_area(
                        &self.season_current_buf,
                        cur_l - 9.0,
                        sc_t - 14.0,
                        w,
                        h,
                        shd,
                    ),
                    // ── Season circle labels — real ──────────────────────────
                    build_label_text_area(
                        &self.lbl_summer,
                        cx - lw / 2.0,
                        cy1 - r - 20.0,
                        w,
                        h,
                        dim,
                    ),
                    build_label_text_area(
                        &self.lbl_winter,
                        cx - lw / 2.0,
                        cy1 + r + 4.0,
                        w,
                        h,
                        dim,
                    ),
                    build_label_text_area(&self.lbl_fall, cx + r + 4.0, cy1 - 8.0, w, h, dim),
                    build_label_text_area(
                        &self.lbl_spring,
                        cx - r - lw - 4.0,
                        cy1 - 8.0,
                        w,
                        h,
                        dim,
                    ),
                    build_label_text_area(
                        &self.season_current_buf,
                        cur_l - 10.0,
                        sc_t - 15.0,
                        w,
                        h,
                        dim,
                    ),
                    // ── Time circle labels — shadows ─────────────────────────
                    build_label_text_area(
                        &self.lbl_12,
                        cx - lw / 2.0 + 1.0,
                        cy2 - r - 19.0,
                        w,
                        h,
                        shd,
                    ),
                    build_label_text_area(
                        &self.lbl_18,
                        cx - lw / 2.0 + 1.0,
                        cy2 + r + 5.0,
                        w,
                        h,
                        shd,
                    ),
                    build_label_text_area(&self.lbl_15, cx + r + 5.0, cy2 - 7.0, w, h, shd),
                    build_label_text_area(&self.lbl_21, cx - r - lw - 3.0, cy2 - 7.0, w, h, shd),
                    build_label_text_area(
                        &self.time_current_buf,
                        cur_l - 9.0,
                        tc_t - 14.0,
                        w,
                        h,
                        shd,
                    ),
                    // ── Time circle labels — real ────────────────────────────
                    build_label_text_area(&self.lbl_12, cx - lw / 2.0, cy2 - r - 20.0, w, h, dim),
                    build_label_text_area(&self.lbl_18, cx - lw / 2.0, cy2 + r + 4.0, w, h, dim),
                    build_label_text_area(&self.lbl_15, cx + r + 4.0, cy2 - 8.0, w, h, dim),
                    build_label_text_area(&self.lbl_21, cx - r - lw - 4.0, cy2 - 8.0, w, h, dim),
                    build_label_text_area(
                        &self.time_current_buf,
                        cur_l - 10.0,
                        tc_t - 15.0,
                        w,
                        h,
                        dim,
                    ),
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
        self.sun_indicator.draw(&mut rpass);
        self.text_renderer
            .render(&self.text_atlas, &self.viewport, &mut rpass)
            .unwrap();
        // begin_render_pass borrows encoder mutably, which means you can't use encoder again until the pass is dropped.
        // The explicit drop(rpass) releases that borrow before encoder.finish().
        drop(rpass);
    }
}
