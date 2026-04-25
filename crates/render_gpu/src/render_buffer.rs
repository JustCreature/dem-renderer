use dem_io::Heightmap;
use terrain::{NormalMap, ShadowMask};
use wgpu::util::DeviceExt;

use crate::camera::CameraUniforms;
use crate::context::GpuContext;

pub fn render_gpu_buffer(
    gpu_ctx: &GpuContext,
    origin: [f32; 3],
    look_at: [f32; 3],
    fov_deg: f32,
    aspect: f32,
    hm: &Heightmap,
    normals: &NormalMap,
    shadow_mask: &ShadowMask,
    sun_dir: [f32; 3],
    width: u32,
    height: u32,
    step_m: f32,
    t_max: f32,
    ao_mode: u32,
) -> Vec<u8> {
    let cam: CameraUniforms = CameraUniforms::new(
        origin, look_at, fov_deg, aspect, hm, sun_dir, width, height, step_m, t_max, ao_mode, 1, 1,
        1, 2,
    );

    // heightmap buffer
    let hm_f32 = &hm.data;
    let hm_buffer: wgpu::Buffer =
        gpu_ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("heightmap"),
                contents: bytemuck::cast_slice(hm_f32),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            });

    let nx_buffer = gpu_ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("nx"),
            contents: bytemuck::cast_slice(&normals.nx),
            usage: wgpu::BufferUsages::STORAGE,
        });

    let ny_buffer = gpu_ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ny"),
            contents: bytemuck::cast_slice(&normals.ny),
            usage: wgpu::BufferUsages::STORAGE,
        });

    let nz_buffer = gpu_ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("nz"),
            contents: bytemuck::cast_slice(&normals.nz),
            usage: wgpu::BufferUsages::STORAGE,
        });

    let shadow_buffer = gpu_ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("shadow_buffer"),
            contents: bytemuck::cast_slice(&shadow_mask.data),
            usage: wgpu::BufferUsages::STORAGE,
        });

    let output_buffer: wgpu::Buffer = gpu_ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("output_buffer"),
        size: (width * height * 4) as u64,
        mapped_at_creation: false,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    });

    let reedback_buffer: wgpu::Buffer = gpu_ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("reedback_buffer"),
        size: (width * height * 4) as u64,
        mapped_at_creation: false,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
    });

    let cam_buffer = gpu_ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("camera"),
            contents: bytemuck::bytes_of(&cam),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

    let bind_group_layout =
        gpu_ctx
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("bgl"),
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
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: false },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
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
                ],
            });

    let bind_group = gpu_ctx
        .device
        .create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bg"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: cam_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: hm_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: output_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: nx_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: ny_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: nz_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: shadow_buffer.as_entire_binding(),
                },
            ],
        });

    let shader = gpu_ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader_buffer.wgsl").into()),
        });

    let pipeline_layout = gpu_ctx
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pipeline_layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

    let pipeline = gpu_ctx
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

    let mut encoder = gpu_ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("encoder"),
        });

    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("compute_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups((width + 7) / 8, (height + 7) / 8, 1);
    }

    encoder.copy_buffer_to_buffer(
        &output_buffer,
        0,
        &reedback_buffer,
        0,
        (width * height * 4) as u64,
    );

    gpu_ctx.queue.submit([encoder.finish()]);

    let (sender, receiver) = std::sync::mpsc::channel();
    reedback_buffer
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

    let data = reedback_buffer.slice(..).get_mapped_range();
    let result: Vec<u8> = data.to_vec();
    drop(data);
    reedback_buffer.unmap();

    result
}
