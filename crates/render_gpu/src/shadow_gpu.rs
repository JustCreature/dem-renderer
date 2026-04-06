use dem_io::Heightmap;
use terrain::ShadowMask;
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ShadowUniforms {
    pub hm_cols:   u32,
    pub hm_rows:   u32,
    pub dx_meters: f32,
    pub tan_sun:   f32,
}

pub fn compute_shadow_gpu(hm: &Heightmap, sun_elevation_rad: f32) -> ShadowMask {
    pollster::block_on(async {
        let instance = wgpu::Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                ..Default::default()
            })
            .await
            .expect("no GPU adapter found");

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .expect("failed to get device");

        let n = hm.rows * hm.cols;
        let buf_size = (n * 4) as u64;

        // uniforms
        let uniforms = ShadowUniforms {
            hm_cols:   hm.cols as u32,
            hm_rows:   hm.rows as u32,
            dx_meters: hm.dx_meters as f32,
            tan_sun:   sun_elevation_rad.tan(),
        };
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label:    Some("shadow_uniforms"),
            contents: bytemuck::bytes_of(&uniforms),
            usage:    wgpu::BufferUsages::UNIFORM,
        });

        // heightmap texture
        let hm_f32: Vec<f32> = hm.data.iter().map(|&v| v as f32).collect();
        let hm_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow_hm_tex"),
            size: wgpu::Extent3d {
                width:                 hm.cols as u32,
                height:                hm.rows as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count:    1,
            dimension:       wgpu::TextureDimension::D2,
            format:          wgpu::TextureFormat::R32Float,
            usage:           wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats:    &[],
        });
        queue.write_texture(
            hm_texture.as_image_copy(),
            bytemuck::cast_slice(&hm_f32),
            wgpu::TexelCopyBufferLayout {
                offset:         0,
                bytes_per_row:  Some(hm.cols as u32 * 4),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width:                 hm.cols as u32,
                height:                hm.rows as u32,
                depth_or_array_layers: 1,
            },
        );
        let hm_view    = hm_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let hm_sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());

        // shadow output buffer — pre-initialised to 1.0 (all lit); shader only writes 0.0
        let shadow_init: Vec<f32> = vec![1.0f32; n];
        let shadow_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label:    Some("shadow"),
            contents: bytemuck::cast_slice(&shadow_init),
            usage:    wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        });

        // readback buffer
        let shadow_rb = device.create_buffer(&wgpu::BufferDescriptor {
            label:              Some("shadow_rb"),
            size:               buf_size,
            mapped_at_creation: false,
            usage:              wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false, min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type:    wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled:   false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false, min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow_bg"),
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniform_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&hm_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&hm_sampler) },
                wgpu::BindGroupEntry { binding: 3, resource: shadow_buf.as_entire_binding() },
            ],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label:  Some("shadow_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader_shadow.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label:              Some("shadow_pipeline_layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size:     0,
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label:               Some("shadow_pipeline"),
            layout:              Some(&pipeline_layout),
            module:              &shader,
            entry_point:         Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache:               None,
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("shadow_encoder"),
        });

        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("shadow_pass"), timestamp_writes: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bg, &[]);
            // one thread per row; workgroup_size is 64
            pass.dispatch_workgroups((hm.rows as u32 + 63) / 64, 1, 1);
        }

        encoder.copy_buffer_to_buffer(&shadow_buf, 0, &shadow_rb, 0, buf_size);
        queue.submit([encoder.finish()]);

        let (sender, receiver) = std::sync::mpsc::channel();
        shadow_rb.slice(..).map_async(wgpu::MapMode::Read, move |r| sender.send(r).unwrap());
        device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
        receiver.recv().unwrap().unwrap();

        let data = {
            let d = shadow_rb.slice(..).get_mapped_range();
            bytemuck::cast_slice::<u8, f32>(&d).to_vec()
        };
        shadow_rb.unmap();

        ShadowMask { data, rows: hm.rows, cols: hm.cols }
    })
}
