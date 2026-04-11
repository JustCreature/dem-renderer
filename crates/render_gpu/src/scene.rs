use dem_io::Heightmap;
use terrain::ShadowMask;
use wgpu::util::DeviceExt;

use crate::camera::CameraUniforms;
use crate::context::GpuContext;
use crate::normals_gpu::NormalsUniforms;

/// Persistent GPU scene: static data uploaded once, only camera uniform
/// written per frame.  Shadow can be updated cheaply via `update_shadow`.
pub struct GpuScene {
    gpu_ctx: GpuContext,

    // Keep alive — bind group holds GPU-side refs but Rust drops CPU-side
    // objects independently, so we must keep them here.
    _hm_texture: wgpu::Texture,
    _hm_view: wgpu::TextureView,
    _hm_sampler: wgpu::Sampler,
    _nx_buf: wgpu::Buffer,
    _ny_buf: wgpu::Buffer,
    _nz_buf: wgpu::Buffer,

    // Mutable per-frame / per-sun-update
    shadow_buf: wgpu::Buffer,
    cam_buf: wgpu::Buffer,

    // Readback path
    output_buf: wgpu::Buffer,
    readback_buf: wgpu::Buffer,

    // Pipeline (compiled once)
    render_pipeline: wgpu::ComputePipeline,
    render_bg: wgpu::BindGroup,

    // Dimensions and terrain scalars needed to build CameraUniforms
    width: u32,
    height: u32,
    hm_cols: u32,
    hm_rows: u32,
    dx_meters: f32,
    dy_meters: f32,
}

impl GpuScene {
    pub fn new(
        gpu_ctx: GpuContext,
        hm: &Heightmap,
        shadow_mask: &ShadowMask,
        width: u32,
        height: u32,
    ) -> Self {
        // heightmap texture
        let hm_f32: Vec<f32> = hm.data.iter().map(|&v| v as f32).collect();
        let hm_texture = gpu_ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scene_hm_tex"),
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

        // normals buffers
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

        // compute normals once, then discard the pipeline
        {
            let nu = NormalsUniforms {
                hm_cols: hm.cols as u32,
                hm_rows: hm.rows as u32,
                dx_meters: hm.dx_meters as f32,
                dy_meters: hm.dy_meters as f32,
            };
            let nu_buf = gpu_ctx
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("normals_ub"),
                    contents: bytemuck::bytes_of(&nu),
                    usage: wgpu::BufferUsages::UNIFORM,
                });
            let bgl = gpu_ctx
                .device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("normals_init_bgl"),
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
                    label: Some("normals_init_bg"),
                    layout: &bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: nu_buf.as_entire_binding(),
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
                    label: Some("normals_init_shader"),
                    source: wgpu::ShaderSource::Wgsl(include_str!("shader_normals.wgsl").into()),
                });
            let pl = gpu_ctx
                .device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("normals_init_pl"),
                    bind_group_layouts: &[Some(&bgl)],
                    immediate_size: 0,
                });
            let pipeline =
                gpu_ctx
                    .device
                    .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                        label: Some("normals_init_pipeline"),
                        layout: Some(&pl),
                        module: &shader,
                        entry_point: Some("main"),
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                        cache: None,
                    });
            let mut enc = gpu_ctx
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("normals_init_enc"),
                });
            {
                let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("normals_init_pass"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &bg, &[]);
                pass.dispatch_workgroups((hm.cols as u32 + 7) / 8, (hm.rows as u32 + 7) / 8, 1);
            }
            gpu_ctx.queue.submit([enc.finish()]);
            // Wait: nx/ny/nz must be filled before the render BG is used.
            gpu_ctx
                .device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .unwrap();
        }

        // shadow buffer (COPY_DST so update_shadow can write_buffer)
        let shadow_buf = gpu_ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("shadow"),
                contents: bytemuck::cast_slice(&shadow_mask.data),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            });

        // camera uniform (128 bytes, overwritten every frame)
        let cam_buf = gpu_ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cam"),
            size: std::mem::size_of::<CameraUniforms>() as u64,
            mapped_at_creation: false,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // output + readback buffers (fixed size, reused every frame)
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

        // render pipeline + bind group (built once, reused every frame)
        let render_bgl =
            gpu_ctx
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
        let render_pl_layout =
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
                    layout: Some(&render_pl_layout),
                    module: &render_shader,
                    entry_point: Some("main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    cache: None,
                });

        GpuScene {
            gpu_ctx,
            _hm_texture: hm_texture,
            _hm_view: hm_view,
            _hm_sampler: hm_sampler,
            _nx_buf: nx_buf,
            _ny_buf: ny_buf,
            _nz_buf: nz_buf,
            shadow_buf,
            cam_buf,
            output_buf,
            readback_buf,
            render_pipeline,
            render_bg,
            width,
            height,
            hm_cols: hm.cols as u32,
            hm_rows: hm.rows as u32,
            dx_meters: hm.dx_meters as f32,
            dy_meters: hm.dy_meters as f32,
        }
    }

    /// Render one frame. Only writes 128 bytes (camera uniform) then dispatches.
    pub fn render_frame(
        &self,
        origin: [f32; 3],
        look_at: [f32; 3],
        fov_deg: f32,
        aspect: f32,
        sun_dir: [f32; 3],
        step_m: f32,
        t_max: f32,
    ) -> Vec<u8> {
        // Build camera uniforms inline (no hm needed — scalars stored in scene)
        let forward = crate::vector_utils::normalize(crate::vector_utils::sub(look_at, origin));
        let right =
            crate::vector_utils::normalize(crate::vector_utils::cross(forward, [0.0, 0.0, 1.0]));
        let up = crate::vector_utils::cross(right, forward);
        let half_w = (fov_deg / 2.0_f32).to_radians().tan();
        let half_h = half_w / aspect;

        let cam = CameraUniforms {
            origin,
            _pad0: 0.0,
            forward,
            _pad1: 0.0,
            right,
            _pad2: 0.0,
            up,
            _pad3: 0.0,
            sun_dir,
            _pad4: 0.0,
            half_w,
            half_h,
            img_width: self.width,
            img_height: self.height,
            hm_cols: self.hm_cols,
            hm_rows: self.hm_rows,
            dx_meters: self.dx_meters,
            dy_meters: self.dy_meters,
            step_m,
            t_max,
            _pad5: 0.0,
            _pad6: 0.0,
            _pad7: 0.0,
            _pad8: 0.0,
            _pad9: 0.0,
            _pad10: 0.0,
        };

        self.gpu_ctx
            .queue
            .write_buffer(&self.cam_buf, 0, bytemuck::bytes_of(&cam));

        let mut encoder =
            self.gpu_ctx
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("render_enc"),
                });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("render_pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.render_pipeline);
            pass.set_bind_group(0, &self.render_bg, &[]);
            pass.dispatch_workgroups((self.width + 7) / 8, (self.height + 7) / 8, 1);
        }
        encoder.copy_buffer_to_buffer(
            &self.output_buf,
            0,
            &self.readback_buf,
            0,
            (self.width * self.height * 4) as u64,
        );
        self.gpu_ctx.queue.submit([encoder.finish()]);

        let (tx, rx) = std::sync::mpsc::channel();
        self.readback_buf
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
        self.gpu_ctx
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .unwrap();
        rx.recv().unwrap().unwrap();

        let result = {
            let d = self.readback_buf.slice(..).get_mapped_range();
            d.to_vec()
        };
        self.readback_buf.unmap();
        result
    }

    /// Dispatches one frame. Only writes 128 bytes (camera uniform) then dispatches.
    pub fn dispatch_frame(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        origin: [f32; 3],
        look_at: [f32; 3],
        fov_deg: f32,
        aspect: f32,
        sun_dir: [f32; 3],
        step_m: f32,
        t_max: f32,
    ) {
        // Build camera uniforms inline (no hm needed — scalars stored in scene)
        let forward = crate::vector_utils::normalize(crate::vector_utils::sub(look_at, origin));
        let right =
            crate::vector_utils::normalize(crate::vector_utils::cross(forward, [0.0, 0.0, 1.0]));
        let up = crate::vector_utils::cross(right, forward);
        let half_w = (fov_deg / 2.0_f32).to_radians().tan();
        let half_h = half_w / aspect;

        let cam = CameraUniforms {
            origin,
            _pad0: 0.0,
            forward,
            _pad1: 0.0,
            right,
            _pad2: 0.0,
            up,
            _pad3: 0.0,
            sun_dir,
            _pad4: 0.0,
            half_w,
            half_h,
            img_width: self.width,
            img_height: self.height,
            hm_cols: self.hm_cols,
            hm_rows: self.hm_rows,
            dx_meters: self.dx_meters,
            dy_meters: self.dy_meters,
            step_m,
            t_max,
            _pad5: 0.0,
            _pad6: 0.0,
            _pad7: 0.0,
            _pad8: 0.0,
            _pad9: 0.0,
            _pad10: 0.0,
        };

        self.gpu_ctx
            .queue
            .write_buffer(&self.cam_buf, 0, bytemuck::bytes_of(&cam));

        // let mut encoder =
        //     self.gpu_ctx
        //         .device
        //         .create_command_encoder(&wgpu::CommandEncoderDescriptor {
        //             label: Some("render_enc"),
        //         });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("render_pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.render_pipeline);
            pass.set_bind_group(0, &self.render_bg, &[]);
            pass.dispatch_workgroups((self.width + 7) / 8, (self.height + 7) / 8, 1);
        }
        // self.gpu_ctx.queue.submit([encoder.finish()]);
    }

    /// Re-upload shadow mask (call when sun direction changes).
    pub fn update_shadow(&self, shadow_mask: &ShadowMask) {
        self.gpu_ctx.queue.write_buffer(
            &self.shadow_buf,
            0,
            bytemuck::cast_slice(&shadow_mask.data),
        );
    }

    pub fn get_output_buffer(&self) -> &wgpu::Buffer {
        &self.output_buf
    }

    pub fn get_gpu_ctx(&self) -> &GpuContext {
        &self.gpu_ctx
    }

    pub fn get_dx_meters(&self) -> f32 {
        self.dx_meters
    }
    pub fn get_dy_meters(&self) -> f32 {
        self.dy_meters
    }
}
