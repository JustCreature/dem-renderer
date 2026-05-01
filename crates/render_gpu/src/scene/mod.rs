mod bind_group;
mod tiers;

use dem_io::Heightmap;
use terrain::{NormalMap, ShadowMask};
use wgpu::util::DeviceExt;

use crate::camera::CameraUniforms;
use crate::context::GpuContext;

/// Persistent GPU scene: static data uploaded once, only camera uniform
/// written per frame.  Shadow can be updated cheaply via `update_shadow`.
pub struct GpuScene {
    pub(super) gpu_ctx: GpuContext,

    // Keep alive — bind group holds GPU-side refs but Rust drops CPU-side
    // objects independently, so we must keep them here.
    pub(super) _hm_texture: wgpu::Texture,
    pub(super) _hm_view: wgpu::TextureView,
    pub(super) _hm_sampler: wgpu::Sampler,
    pub(super) _nx_buf: wgpu::Buffer,
    pub(super) _ny_buf: wgpu::Buffer,
    pub(super) _nz_buf: wgpu::Buffer,
    // AO
    pub(super) _ao_texture: wgpu::Texture,
    pub(super) _ao_view: wgpu::TextureView,
    pub(super) _ao_sampler: wgpu::Sampler,

    // hm5m close tier (placeholder until upload_hm5m; extent_x==0 means inactive)
    pub(super) _hm5m_texture: wgpu::Texture,
    pub(super) _hm5m_view: wgpu::TextureView,
    pub(super) _hm5m_sampler: wgpu::Sampler,
    pub(super) _hm5m_normal_tex: wgpu::Texture,
    pub(super) _hm5m_normal_view: wgpu::TextureView,
    pub(super) _hm5m_normal_sampler: wgpu::Sampler,
    pub(super) _hm5m_shadow_buf: wgpu::Buffer,
    pub(super) hm5m_origin_x: f32,
    pub(super) hm5m_origin_y: f32,
    pub(super) hm5m_extent_x: f32,
    pub(super) hm5m_extent_y: f32,
    pub(super) hm5m_cols: u32,
    pub(super) hm5m_rows: u32,
    pub(super) hm5m_buf_elems: u64,

    // hm1m fine tier (placeholder until upload_hm1m; extent_x==0 means inactive)
    pub(super) _hm1m_texture: wgpu::Texture,
    pub(super) _hm1m_view: wgpu::TextureView,
    pub(super) _hm1m_sampler: wgpu::Sampler,
    pub(super) _hm1m_normal_tex: wgpu::Texture,
    pub(super) _hm1m_normal_view: wgpu::TextureView,
    pub(super) _hm1m_normal_sampler: wgpu::Sampler,
    pub(super) _hm1m_shadow_buf: wgpu::Buffer,
    pub(super) hm1m_origin_x: f32,
    pub(super) hm1m_origin_y: f32,
    pub(super) hm1m_extent_x: f32,
    pub(super) hm1m_extent_y: f32,
    pub(super) hm1m_cols: u32,
    pub(super) hm1m_rows: u32,
    pub(super) hm1m_buf_elems: u64,

    // Mutable per-frame / per-sun-update
    pub(super) shadow_buf: wgpu::Buffer,
    pub(super) cam_buf: wgpu::Buffer,

    // Readback path
    pub(super) output_buf: wgpu::Buffer,
    pub(super) readback_buf: wgpu::Buffer,

    // Pipeline (compiled once)
    pub(super) render_pipeline: wgpu::ComputePipeline,
    pub(super) render_bg: wgpu::BindGroup,
    pub(super) render_bgl: wgpu::BindGroupLayout,

    // Dimensions and terrain scalars needed to build CameraUniforms
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) hm_cols: u32,
    pub(super) hm_rows: u32,
    pub(super) dx_meters: f32,
    pub(super) dy_meters: f32,
}

/// Create a 1×1 R16Float placeholder texture + Linear sampler + 4 × 1-element f32 storage
/// buffers for a tier slot. Used to initialise hm5m and hm1m before real data arrives.
pub(super) fn create_tier_placeholder(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
) -> (
    wgpu::Texture,
    wgpu::TextureView,
    wgpu::Sampler,
    wgpu::Texture, // normal tex (Rgba8Snorm)
    wgpu::TextureView,
    wgpu::Sampler,
    wgpu::Buffer, // shadow buf
) {
    // Heightmap placeholder: R16Float 1×1
    let ph_tex_data: [half::f16; 1] = [half::f16::from_f32(0.0)];
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(&format!("{}_tex", label)),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        bytemuck::cast_slice(&ph_tex_data),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(2),
            rows_per_image: None,
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    // Normal placeholder: Rgba8Snorm 1×1, [0, 0] decodes to (x=0, y=0) → z=1 (up normal)
    let ph_normal_data: [i8; 4] = [0, 0, 0, 0];
    let normal_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(&format!("{}_normal_tex", label)),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Snorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        normal_tex.as_image_copy(),
        bytemuck::cast_slice(&ph_normal_data),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4),
            rows_per_image: None,
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    let normal_view = normal_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let normal_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    // Shadow placeholder: 1-element f32 buffer
    let ph_buf_data: [f32; 1] = [0.0];
    let shadow_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(&format!("{}_shadow", label)),
        contents: bytemuck::cast_slice(&ph_buf_data),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });
    (
        texture,
        view,
        sampler,
        normal_tex,
        normal_view,
        normal_sampler,
        shadow_buf,
    )
}

/// Generate mip levels 1..7 for a heightmap texture using a max filter.
pub(super) fn write_hm_mips(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    base_data: &[half::f16],
    cols: usize,
    rows: usize,
) {
    let mut prev_data: Vec<half::f16> = base_data.to_vec();
    let mut prev_w = cols;
    let mut prev_h = rows;
    for mip in 1u32..8u32 {
        let w = (prev_w / 2).max(1);
        let h = (prev_h / 2).max(1);
        let mut mip_data: Vec<half::f16> = Vec::with_capacity(w * h);
        for row in 0..h {
            for col in 0..w {
                let r0 = (row * 2).min(prev_h - 1);
                let r1 = (row * 2 + 1).min(prev_h - 1);
                let c0 = (col * 2).min(prev_w - 1);
                let c1 = (col * 2 + 1).min(prev_w - 1);
                let a = prev_data[r0 * prev_w + c0].to_f32();
                let b = prev_data[r0 * prev_w + c1].to_f32();
                let c = prev_data[r1 * prev_w + c0].to_f32();
                let d = prev_data[r1 * prev_w + c1].to_f32();
                mip_data.push(half::f16::from_f32(a.max(b).max(c).max(d)));
            }
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: mip,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&mip_data),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w as u32 * 2),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: w as u32,
                height: h as u32,
                depth_or_array_layers: 1,
            },
        );
        prev_data = mip_data;
        prev_w = w;
        prev_h = h;
    }
}

impl GpuScene {
    pub fn new(
        gpu_ctx: GpuContext,
        hm: &Heightmap,
        normal_map: &NormalMap,
        shadow_mask: &ShadowMask,
        ao_data_mask: &Vec<f32>,
        width: u32,
        height: u32,
    ) -> Self {
        // heightmap texture
        let hm_data: Vec<half::f16> = hm.data.iter().map(|&v| half::f16::from_f32(v)).collect();
        let hm_texture = gpu_ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scene_hm_tex"),
            size: wgpu::Extent3d {
                width: hm.cols as u32,
                height: hm.rows as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 8,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        gpu_ctx.queue.write_texture(
            hm_texture.as_image_copy(),
            bytemuck::cast_slice(&hm_data),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(hm.cols as u32 * 2),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: hm.cols as u32,
                height: hm.rows as u32,
                depth_or_array_layers: 1,
            },
        );
        write_hm_mips(&gpu_ctx.queue, &hm_texture, &hm_data, hm.cols, hm.rows);

        let hm_view = hm_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let hm_sampler = gpu_ctx.device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });

        // AO
        let ao_data: Vec<u8> = ao_data_mask
            .iter()
            .map(|&v| (v * 255.0) as u8)
            .collect::<Vec<u8>>();
        let ao_texture = gpu_ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scene_ao_tex"),
            size: wgpu::Extent3d {
                width: hm.cols as u32,
                height: hm.rows as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        gpu_ctx.queue.write_texture(
            ao_texture.as_image_copy(),
            bytemuck::cast_slice(&ao_data),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(hm.cols as u32),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: hm.cols as u32,
                height: hm.rows as u32,
                depth_or_array_layers: 1,
            },
        );
        let ao_view = ao_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let ao_sampler = gpu_ctx.device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // hm5m + hm1m placeholders (1×1 R16Float, 1-element buffers) — inactive until upload
        let (
            hm5m_texture,
            hm5m_view,
            hm5m_sampler,
            hm5m_normal_tex,
            hm5m_normal_view,
            hm5m_normal_sampler,
            hm5m_shadow_buf,
        ) = create_tier_placeholder(&gpu_ctx.device, &gpu_ctx.queue, "hm5m");
        let (
            hm1m_texture,
            hm1m_view,
            hm1m_sampler,
            hm1m_normal_tex,
            hm1m_normal_view,
            hm1m_normal_sampler,
            hm1m_shadow_buf,
        ) = create_tier_placeholder(&gpu_ctx.device, &gpu_ctx.queue, "hm1m");

        // normals buffers — COPY_DST so update_heightmap can write_buffer
        let nx_buf = gpu_ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("nx"),
                contents: bytemuck::cast_slice(&normal_map.nx),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            });
        let ny_buf = gpu_ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("ny"),
                contents: bytemuck::cast_slice(&normal_map.ny),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            });
        let nz_buf = gpu_ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("nz"),
                contents: bytemuck::cast_slice(&normal_map.nz),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            });

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
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 2,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
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
                        wgpu::BindGroupLayoutEntry {
                            binding: 8,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 9,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                            count: None,
                        },
                        // hm5m close tier
                        wgpu::BindGroupLayoutEntry {
                            binding: 10,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 11,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 12,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 13,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 14,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        // hm1m fine tier
                        wgpu::BindGroupLayoutEntry {
                            binding: 15,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 16,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 17,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 18,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 19,
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
                    // hm
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
                    // ao
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: wgpu::BindingResource::TextureView(&ao_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 9,
                        resource: wgpu::BindingResource::Sampler(&ao_sampler),
                    },
                    // hm5m close tier (placeholder)
                    wgpu::BindGroupEntry {
                        binding: 10,
                        resource: wgpu::BindingResource::TextureView(&hm5m_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 11,
                        resource: wgpu::BindingResource::Sampler(&hm5m_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 12,
                        resource: wgpu::BindingResource::TextureView(&hm5m_normal_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 13,
                        resource: wgpu::BindingResource::Sampler(&hm5m_normal_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 14,
                        resource: hm5m_shadow_buf.as_entire_binding(),
                    },
                    // hm1m fine tier (placeholder)
                    wgpu::BindGroupEntry {
                        binding: 15,
                        resource: wgpu::BindingResource::TextureView(&hm1m_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 16,
                        resource: wgpu::BindingResource::Sampler(&hm1m_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 17,
                        resource: wgpu::BindingResource::TextureView(&hm1m_normal_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 18,
                        resource: wgpu::BindingResource::Sampler(&hm1m_normal_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 19,
                        resource: hm1m_shadow_buf.as_entire_binding(),
                    },
                ],
            });
        let render_shader = gpu_ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("render_shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../shader_texture.wgsl").into()),
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
            _ao_texture: ao_texture,
            _ao_view: ao_view,
            _ao_sampler: ao_sampler,
            _hm5m_texture: hm5m_texture,
            _hm5m_view: hm5m_view,
            _hm5m_sampler: hm5m_sampler,
            _hm5m_normal_tex: hm5m_normal_tex,
            _hm5m_normal_view: hm5m_normal_view,
            _hm5m_normal_sampler: hm5m_normal_sampler,
            _hm5m_shadow_buf: hm5m_shadow_buf,
            hm5m_origin_x: 0.0,
            hm5m_origin_y: 0.0,
            hm5m_extent_x: 0.0,
            hm5m_extent_y: 0.0,
            hm5m_cols: 0,
            hm5m_rows: 0,
            hm5m_buf_elems: 1,
            _hm1m_texture: hm1m_texture,
            _hm1m_view: hm1m_view,
            _hm1m_sampler: hm1m_sampler,
            _hm1m_normal_tex: hm1m_normal_tex,
            _hm1m_normal_view: hm1m_normal_view,
            _hm1m_normal_sampler: hm1m_normal_sampler,
            _hm1m_shadow_buf: hm1m_shadow_buf,
            hm1m_origin_x: 0.0,
            hm1m_origin_y: 0.0,
            hm1m_extent_x: 0.0,
            hm1m_extent_y: 0.0,
            hm1m_cols: 0,
            hm1m_rows: 0,
            hm1m_buf_elems: 1,
            shadow_buf,
            cam_buf,
            output_buf,
            readback_buf,
            render_pipeline,
            render_bg,
            render_bgl,
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
        ao_mode: u32,
        shadows_enabled: u32,
        fog_enabled: u32,
        vat_mode: u32,
        lod_mode: u32,
    ) -> Vec<u8> {
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
            ao_mode,
            _pad5: 0.0,
            shadows_enabled,
            fog_enabled,
            vat_mode,
            lod_mode,
            hm5m_origin_x: self.hm5m_origin_x,
            hm5m_origin_y: self.hm5m_origin_y,
            hm5m_extent_x: self.hm5m_extent_x,
            hm5m_extent_y: self.hm5m_extent_y,
            hm5m_cols: self.hm5m_cols,
            hm5m_rows: self.hm5m_rows,
            _pad6: 0,
            _pad7: 0,
            hm1m_origin_x: self.hm1m_origin_x,
            hm1m_origin_y: self.hm1m_origin_y,
            hm1m_extent_x: self.hm1m_extent_x,
            hm1m_extent_y: self.hm1m_extent_y,
            hm1m_cols: self.hm1m_cols,
            hm1m_rows: self.hm1m_rows,
            _pad8: 0,
            _pad9: 0,
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
        ao_mode: u32,
        shadows_enabled: u32,
        fog_enabled: u32,
        vat_mode: u32,
        lod_mode: u32,
    ) {
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
            ao_mode,
            _pad5: 0.0,
            shadows_enabled,
            fog_enabled,
            vat_mode,
            lod_mode,
            hm5m_origin_x: self.hm5m_origin_x,
            hm5m_origin_y: self.hm5m_origin_y,
            hm5m_extent_x: self.hm5m_extent_x,
            hm5m_extent_y: self.hm5m_extent_y,
            hm5m_cols: self.hm5m_cols,
            hm5m_rows: self.hm5m_rows,
            _pad6: 0,
            _pad7: 0,
            hm1m_origin_x: self.hm1m_origin_x,
            hm1m_origin_y: self.hm1m_origin_y,
            hm1m_extent_x: self.hm1m_extent_x,
            hm1m_extent_y: self.hm1m_extent_y,
            hm1m_cols: self.hm1m_cols,
            hm1m_rows: self.hm1m_rows,
            _pad8: 0,
            _pad9: 0,
        };

        self.gpu_ctx
            .queue
            .write_buffer(&self.cam_buf, 0, bytemuck::bytes_of(&cam));

        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("render_pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.render_pipeline);
            pass.set_bind_group(0, &self.render_bg, &[]);
            pass.dispatch_workgroups((self.width + 7) / 8, (self.height + 7) / 8, 1);
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;

        self.output_buf = self.gpu_ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("output"),
            size: (width * height * 4) as u64,
            mapped_at_creation: false,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        });

        self.readback_buf = self.gpu_ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (width * height * 4) as u64,
            mapped_at_creation: false,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        });

        self.rebuild_bind_group();
    }

    /// Re-upload shadow mask (call when sun direction changes).
    pub fn update_ao(&self, ao_data_mask: &[f32]) {
        let ao_data: Vec<u8> = ao_data_mask.iter().map(|&v| (v * 255.0) as u8).collect();
        self.gpu_ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self._ao_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&ao_data),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.hm_cols),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: self.hm_cols,
                height: self.hm_rows,
                depth_or_array_layers: 1,
            },
        );
    }

    pub fn update_shadow(&self, shadow_mask: &ShadowMask) {
        self.gpu_ctx.queue.write_buffer(
            &self.shadow_buf,
            0,
            bytemuck::cast_slice(&shadow_mask.data),
        );
    }

    /// Re-upload heightmap, normals, and AO after a tile slide.
    /// Existing GPU textures/buffers are reused in-place; bind group is unchanged.
    pub fn update_heightmap(
        &mut self,
        hm: &Heightmap,
        normal_map: &NormalMap,
        ao_data_mask: &[f32],
    ) {
        let hm_data: Vec<half::f16> = hm.data.iter().map(|&v| half::f16::from_f32(v)).collect();
        self.gpu_ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self._hm_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&hm_data),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(hm.cols as u32 * 2),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: hm.cols as u32,
                height: hm.rows as u32,
                depth_or_array_layers: 1,
            },
        );
        write_hm_mips(
            &self.gpu_ctx.queue,
            &self._hm_texture,
            &hm_data,
            hm.cols,
            hm.rows,
        );

        self.gpu_ctx
            .queue
            .write_buffer(&self._nx_buf, 0, bytemuck::cast_slice(&normal_map.nx));
        self.gpu_ctx
            .queue
            .write_buffer(&self._ny_buf, 0, bytemuck::cast_slice(&normal_map.ny));
        self.gpu_ctx
            .queue
            .write_buffer(&self._nz_buf, 0, bytemuck::cast_slice(&normal_map.nz));

        let ao_data: Vec<u8> = ao_data_mask.iter().map(|&v| (v * 255.0) as u8).collect();
        self.gpu_ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self._ao_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&ao_data),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(hm.cols as u32),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: hm.cols as u32,
                height: hm.rows as u32,
                depth_or_array_layers: 1,
            },
        );

        self.hm_cols = hm.cols as u32;
        self.hm_rows = hm.rows as u32;
        self.dx_meters = hm.dx_meters as f32;
        self.dy_meters = hm.dy_meters as f32;
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
