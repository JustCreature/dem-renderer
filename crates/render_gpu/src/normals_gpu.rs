use dem_io::Heightmap;
use terrain::NormalMap;
use wgpu::util::DeviceExt;

use crate::context::GpuContext;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct NormalsUniforms {
    pub hm_cols: u32,
    pub hm_rows: u32,
    pub dx_meters: f32,
    pub dy_meters: f32,
}

pub fn compute_normals_gpu(gpu_ctx: &GpuContext, hm: &Heightmap) -> NormalMap {
    let n = hm.rows * hm.cols;
    let buf_size = (n * 4) as u64;

    let uniforms = NormalsUniforms {
        hm_cols: hm.cols as u32,
        hm_rows: hm.rows as u32,
        dx_meters: hm.dx_meters as f32,
        dy_meters: hm.dy_meters as f32,
    };
    let uniform_buffer = gpu_ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("normals_uniforms"),
            contents: bytemuck::bytes_of(&uniforms),
            usage: wgpu::BufferUsages::UNIFORM,
        });

    let hm_f32: Vec<f32> = hm.data.iter().map(|&v| v as f32).collect();
    let hm_texture = gpu_ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("normals_hm_tex"),
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

    let nx_buf = gpu_ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("nx"),
        size: buf_size,
        mapped_at_creation: false,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    });
    let ny_buf = gpu_ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ny"),
        size: buf_size,
        mapped_at_creation: false,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    });
    let nz_buf = gpu_ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("nz"),
        size: buf_size,
        mapped_at_creation: false,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    });
    let nx_rb = gpu_ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("nx_rb"),
        size: buf_size,
        mapped_at_creation: false,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
    });
    let ny_rb = gpu_ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ny_rb"),
        size: buf_size,
        mapped_at_creation: false,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
    });
    let nz_rb = gpu_ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("nz_rb"),
        size: buf_size,
        mapped_at_creation: false,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
    });

    let bgl = gpu_ctx
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
    let bg = gpu_ctx
        .device
        .create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("normals_bg"),
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
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

    let shader = gpu_ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("normals_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader_normals.wgsl").into()),
        });
    let pl = gpu_ctx
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("normals_pl"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
    let pipeline = gpu_ctx
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("normals_pipeline"),
            layout: Some(&pl),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

    let mut encoder = gpu_ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("normals_encoder"),
        });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("normals_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bg, &[]);
        pass.dispatch_workgroups((hm.cols as u32 + 7) / 8, (hm.rows as u32 + 7) / 8, 1);
    }
    encoder.copy_buffer_to_buffer(&nx_buf, 0, &nx_rb, 0, buf_size);
    encoder.copy_buffer_to_buffer(&ny_buf, 0, &ny_rb, 0, buf_size);
    encoder.copy_buffer_to_buffer(&nz_buf, 0, &nz_rb, 0, buf_size);
    gpu_ctx.queue.submit([encoder.finish()]);

    let (sender, receiver) = std::sync::mpsc::channel();
    nx_rb.slice(..).map_async(wgpu::MapMode::Read, {
        let s = sender.clone();
        move |r| s.send(r).unwrap()
    });
    ny_rb.slice(..).map_async(wgpu::MapMode::Read, {
        let s = sender.clone();
        move |r| s.send(r).unwrap()
    });
    nz_rb
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
    receiver.recv().unwrap().unwrap();
    receiver.recv().unwrap().unwrap();

    let nx = {
        let d = nx_rb.slice(..).get_mapped_range();
        bytemuck::cast_slice::<u8, f32>(&d).to_vec()
    };
    let ny = {
        let d = ny_rb.slice(..).get_mapped_range();
        bytemuck::cast_slice::<u8, f32>(&d).to_vec()
    };
    let nz = {
        let d = nz_rb.slice(..).get_mapped_range();
        bytemuck::cast_slice::<u8, f32>(&d).to_vec()
    };
    nx_rb.unmap();
    ny_rb.unmap();
    nz_rb.unmap();

    NormalMap {
        nx,
        ny,
        nz,
        rows: hm.rows,
        cols: hm.cols,
    }
}
