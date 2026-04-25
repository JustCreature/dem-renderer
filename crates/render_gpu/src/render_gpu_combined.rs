use dem_io::Heightmap;
use terrain::ShadowMask;
use wgpu::util::DeviceExt;

use crate::camera::CameraUniforms;
use crate::context::GpuContext;
use crate::normals_gpu::NormalsUniforms;

pub fn render_gpu_combined(
    gpu_ctx: &GpuContext,
    hm: &Heightmap,
    shadow_mask: &ShadowMask,
    origin: [f32; 3],
    look_at: [f32; 3],
    fov_deg: f32,
    aspect: f32,
    sun_dir: [f32; 3],
    width: u32,
    height: u32,
    step_m: f32,
    t_max: f32,
) -> Vec<u8> {
    // ── heightmap texture (shared between normals pass and render pass) ───
    let hm_f32: Vec<f32> = hm.data.iter().map(|&v| v as f32).collect();
    let hm_texture = gpu_ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("hm_tex"),
        size: wgpu::Extent3d {
            width: hm.cols as u32,
            height: hm.rows as u32,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    gpu_ctx.queue.write_texture(
        hm_texture.as_image_copy(),
        bytemuck::cast_slice(&hm_f32),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(hm.cols as u32 * 4),
            rows_per_image: None,
        },
        wgpu::Extent3d {
            width: hm.cols as u32,
            height: hm.rows as u32,
            depth_or_array_layers: 1,
        },
    );
    let hm_view = hm_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let hm_sampler = gpu_ctx
        .device
        .create_sampler(&wgpu::SamplerDescriptor::default());

    // ── normals output buffers (stay on GPU — no readback, no COPY_SRC) ──
    let nm_size = (hm.rows * hm.cols * 4) as u64;
    let nx_buf = gpu_ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("nx"),
        size: nm_size,
        mapped_at_creation: false,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let ny_buf = gpu_ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ny"),
        size: nm_size,
        mapped_at_creation: false,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let nz_buf = gpu_ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("nz"),
        size: nm_size,
        mapped_at_creation: false,
        usage: wgpu::BufferUsages::STORAGE,
    });

    // ── normals pipeline ──────────────────────────────────────────────────
    let normals_uniforms = NormalsUniforms {
        hm_cols: hm.cols as u32,
        hm_rows: hm.rows as u32,
        dx_meters: hm.dx_meters as f32,
        dy_meters: hm.dy_meters as f32,
    };
    let normals_ub = gpu_ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("normals_ub"),
            contents: bytemuck::bytes_of(&normals_uniforms),
            usage: wgpu::BufferUsages::UNIFORM,
        });

    let normals_bgl = gpu_ctx
        .device
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("normals_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

    let normals_bg = gpu_ctx
        .device
        .create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("normals_bg"),
            layout: &normals_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: normals_ub.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&hm_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&hm_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: nx_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: ny_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: nz_buf.as_entire_binding(),
                },
            ],
        });

    let normals_shader = gpu_ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("normals_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader_normals.wgsl").into()),
        });
    let normals_pipeline_layout =
        gpu_ctx
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("normals_pl"),
                bind_group_layouts: &[Some(&normals_bgl)],
                immediate_size: 0,
            });
    let normals_pipeline =
        gpu_ctx
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("normals_pipeline"),
                layout: Some(&normals_pipeline_layout),
                module: &normals_shader,
                entry_point: Some("main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            });

    // ── render pipeline ───────────────────────────────────────────────────
    let cam = CameraUniforms::new(
        origin, look_at, fov_deg, aspect, hm, sun_dir, width, height, step_m, t_max,
    );
    let cam_buf = gpu_ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("cam"),
            contents: bytemuck::bytes_of(&cam),
            usage: wgpu::BufferUsages::UNIFORM,
        });

    let shadow_buf = gpu_ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("shadow"),
            contents: bytemuck::cast_slice(&shadow_mask.data),
            usage: wgpu::BufferUsages::STORAGE,
        });

    let output_buf = gpu_ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("output"),
        size: (width * height * 4) as u64,
        mapped_at_creation: false,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    });
    let readback_buf = gpu_ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (width * height * 4) as u64,
        mapped_at_creation: false,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
    });

    let render_bgl = gpu_ctx
        .device
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("render_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

    let render_bg = gpu_ctx
        .device
        .create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("render_bg"),
            layout: &render_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: cam_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&hm_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&hm_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: output_buf.as_entire_binding(),
                },
                // nx/ny/nz come directly from the normals pass — no CPU round-trip
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: nx_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: ny_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: nz_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: shadow_buf.as_entire_binding(),
                },
            ],
        });

    let render_shader = gpu_ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("render_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader_texture.wgsl").into()),
        });
    let render_pipeline_layout =
        gpu_ctx
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("render_pl"),
                bind_group_layouts: &[Some(&render_bgl)],
                immediate_size: 0,
            });
    let render_pipeline =
        gpu_ctx
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("render_pipeline"),
                layout: Some(&render_pipeline_layout),
                module: &render_shader,
                entry_point: Some("main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            });

    // ── encode both passes into one command buffer ────────────────────────
    let mut encoder = gpu_ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("combined_encoder"),
        });

    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("normals_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&normals_pipeline);
        pass.set_bind_group(0, &normals_bg, &[]);
        pass.dispatch_workgroups((hm.cols as u32 + 7) / 8, (hm.rows as u32 + 7) / 8, 1);
    }

    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("render_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&render_pipeline);
        pass.set_bind_group(0, &render_bg, &[]);
        pass.dispatch_workgroups((width + 7) / 8, (height + 7) / 8, 1);
    }

    encoder.copy_buffer_to_buffer(
        &output_buf,
        0,
        &readback_buf,
        0,
        (width * height * 4) as u64,
    );

    gpu_ctx.queue.submit([encoder.finish()]);

    // ── readback image only ───────────────────────────────────────────────
    let (sender, receiver) = std::sync::mpsc::channel();
    readback_buf
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |r| sender.send(r).unwrap());
    gpu_ctx
        .device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .unwrap();
    receiver.recv().unwrap().unwrap();

    let result = {
        let d = readback_buf.slice(..).get_mapped_range();
        d.to_vec()
    };
    readback_buf.unmap();
    result
}
