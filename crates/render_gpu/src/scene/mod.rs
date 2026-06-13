mod bind_group;
mod tiers;

use dem_io::Heightmap;
use terrain::{NormalMap, ShadowMask};

use crate::camera::CameraUniforms;
use crate::context::GpuContext;
use crate::vram;

/// Persistent GPU scene: static data uploaded once, only camera uniform
/// written per frame.  Shadow can be updated cheaply via `update_shadow`.
pub struct GpuScene {
    pub(super) gpu_ctx: GpuContext,

    // Keep alive — bind group holds GPU-side refs but Rust drops CPU-side
    // objects independently, so we must keep them here.
    pub(super) _hm_texture: wgpu::Texture,
    pub(super) _hm_view: wgpu::TextureView,
    pub(super) _hm_sampler: wgpu::Sampler,
    pub(super) _normals_packed_buf: wgpu::Buffer,
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
    pub(super) hm5m_cos_rot: f32,
    pub(super) hm5m_sin_rot: f32,
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
    pub(super) hm1m_cos_rot: f32,
    pub(super) hm1m_sin_rot: f32,
    pub(super) hm1m_buf_elems: u64,

    // ortho albedo windows (placeholder until upload_ortho_*; extent_x==0 = inactive)
    pub(super) _ortho_fine_tex: wgpu::Texture,
    pub(super) _ortho_fine_view: wgpu::TextureView,
    pub(super) _ortho_fine_sampler: wgpu::Sampler,
    pub(super) ortho_fine_origin_x: f32,
    pub(super) ortho_fine_origin_y: f32,
    pub(super) ortho_fine_extent_x: f32,
    pub(super) ortho_fine_extent_y: f32,
    pub(super) ortho_fine_cos_rot: f32,
    pub(super) ortho_fine_sin_rot: f32,
    pub(super) ortho_fine_cols: u32,
    pub(super) ortho_fine_rows: u32,

    pub(super) _ortho_close_tex: wgpu::Texture,
    pub(super) _ortho_close_view: wgpu::TextureView,
    pub(super) _ortho_close_sampler: wgpu::Sampler,
    pub(super) ortho_close_origin_x: f32,
    pub(super) ortho_close_origin_y: f32,
    pub(super) ortho_close_extent_x: f32,
    pub(super) ortho_close_extent_y: f32,
    pub(super) ortho_close_cos_rot: f32,
    pub(super) ortho_close_sin_rot: f32,
    pub(super) ortho_close_cols: u32,
    pub(super) ortho_close_rows: u32,

    // Mutable per-frame / per-sun-update
    pub(super) shadow_buf: wgpu::Buffer,
    pub(super) cam_buf: wgpu::Buffer,

    // Readback path
    pub(super) output_buf: wgpu::Buffer,

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
    pub(super) max_terrain_h: f32,
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
    let tex_label_owned = format!("{}_tex", label);
    let texture = vram::create_texture_tracked(
        device,
        &wgpu::TextureDescriptor {
            label: Some(&tex_label_owned),
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
        },
        &tex_label_owned,
    );
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
    let normal_label_owned = format!("{}_normal_tex", label);
    let normal_tex = vram::create_texture_tracked(
        device,
        &wgpu::TextureDescriptor {
            label: Some(&normal_label_owned),
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
        },
        &normal_label_owned,
    );
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
    let shadow_label_owned = format!("{}_shadow", label);
    let shadow_buf = vram::create_buffer_init_tracked(
        device,
        &wgpu::util::BufferInitDescriptor {
            label: Some(&shadow_label_owned),
            contents: bytemuck::cast_slice(&ph_buf_data),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        },
        &shadow_label_owned,
    );
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

/// Build the three size-tied placeholder resources (1×1 hm texture, 1×1 normal texture,
/// 1-element shadow buffer) for a close/fine tier. Used by the drop-first reload cycle
/// to release wgpu's Arc on the previous large resources before allocating new ones —
/// keeping reload peak memory close to `max(old, new)` instead of `old + new`.
///
/// Samplers are not regenerated; the bind group keeps using the originals.
pub(super) fn make_tier_size_placeholders(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
) -> (
    wgpu::Texture,
    wgpu::TextureView,
    wgpu::Texture,
    wgpu::TextureView,
    wgpu::Buffer,
) {
    let tex_label = format!("{}_tex", label);
    let texture = vram::create_texture_tracked(
        device,
        &wgpu::TextureDescriptor {
            label: Some(&tex_label),
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
        },
        &tex_label,
    );
    let ph_tex_data: [half::f16; 1] = [half::f16::from_f32(0.0)];
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

    let normal_label = format!("{}_normal_tex", label);
    let normal_tex = vram::create_texture_tracked(
        device,
        &wgpu::TextureDescriptor {
            label: Some(&normal_label),
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
        },
        &normal_label,
    );
    let ph_normal_data: [i8; 4] = [0, 0, 0, 0];
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

    use wgpu::util::DeviceExt;
    let buf_label = format!("{}_shadow", label);
    let ph_buf_data: [f32; 1] = [0.0];
    let bytes = bytemuck::cast_slice::<f32, u8>(&ph_buf_data).len() as u64;
    vram::GPU_BUFFER_BYTES.fetch_add(bytes, std::sync::atomic::Ordering::Relaxed);
    let shadow_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(&buf_label),
        contents: bytemuck::cast_slice(&ph_buf_data),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });
    eprintln!(
        "[vram] alloc buf {:<22} +   0.00 MB  (tier placeholder swap)",
        buf_label
    );

    (texture, view, normal_tex, normal_view, shadow_buf)
}

/// 1×1 Rgba8Unorm placeholder texture for an ortho albedo slot. Used both at
/// scene creation and by the drop-first reload cycle (`upload_ortho_*` /
/// `set_ortho_*_inactive`) — the existing `make_tier_size_placeholders` is
/// hardcoded to the height-tier resource trio (R16Float + Rgba8Snorm + shadow
/// buffer), so the ortho slot gets its own minimal variant.
pub(super) fn make_ortho_placeholder(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
) -> (wgpu::Texture, wgpu::TextureView) {
    let tex_label = format!("{}_tex", label);
    let texture = vram::create_texture_tracked(
        device,
        &wgpu::TextureDescriptor {
            label: Some(&tex_label),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        &tex_label,
    );
    queue.write_texture(
        texture.as_image_copy(),
        &[0u8, 0, 0, 0],
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
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// Generate mip levels 1..7 for a heightmap texture using a max filter.
pub(super) fn write_hm_mips(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    mips: &[(u32, u32, Vec<u8>)],
) {
    for (mip_idx, (w, h, data)) in mips.iter().enumerate() {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: (mip_idx + 1) as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 2),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: *w,
                height: *h,
                depth_or_array_layers: 1,
            },
        );
    }
}

/// Write pre-generated RGBA8 mip levels 1..N (4 bytes/texel).
pub(super) fn write_rgba_mips(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    mips: &[(u32, u32, Vec<u8>)],
) {
    for (mip_idx, (w, h, data)) in mips.iter().enumerate() {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: (mip_idx + 1) as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: *w,
                height: *h,
                depth_or_array_layers: 1,
            },
        );
    }
}

impl GpuScene {
    pub fn new(
        gpu_ctx: GpuContext,
        hm: &Heightmap,
        normal_map: &NormalMap,
        shadow_mask: &ShadowMask,
        ao_data_mask: &[f32],
        width: u32,
        height: u32,
    ) -> Self {
        // heightmap texture
        let hm_data: Vec<half::f16> = hm.data.iter().map(|&v| half::f16::from_f32(v)).collect();
        let hm_texture = vram::create_texture_tracked(
            &gpu_ctx.device,
            &wgpu::TextureDescriptor {
                label: Some("scene_hm_tex"),
                size: wgpu::Extent3d {
                    width: hm.cols as u32,
                    height: hm.rows as u32,
                    depth_or_array_layers: 1,
                },
                mip_level_count: crate::hm_mip_count(hm.cols as u32, hm.rows as u32),
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            "scene_hm_tex",
        );
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
        let hm_mip_bytes =
            crate::gen_hm_mip_bytes(bytemuck::cast_slice(&hm_data), hm.cols, hm.rows);
        write_hm_mips(&gpu_ctx.queue, &hm_texture, &hm_mip_bytes);

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
        let ao_texture = vram::create_texture_tracked(
            &gpu_ctx.device,
            &wgpu::TextureDescriptor {
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
            },
            "scene_ao_tex",
        );
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

        // ortho albedo placeholders (1×1 Rgba8Unorm) — inactive until upload_ortho_*
        let (ortho_fine_tex, ortho_fine_view) =
            make_ortho_placeholder(&gpu_ctx.device, &gpu_ctx.queue, "ortho_fine");
        let (ortho_close_tex, ortho_close_view) =
            make_ortho_placeholder(&gpu_ctx.device, &gpu_ctx.queue, "ortho_close");
        let mk_ortho_sampler = || {
            gpu_ctx.device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..Default::default()
            })
        };
        let ortho_fine_sampler = mk_ortho_sampler();
        let ortho_close_sampler = mk_ortho_sampler();

        // normals packed buffer: bits 31–16 = nx_i16, bits 15–0 = ny_i16; nz reconstructed in shader
        // COPY_DST so update_heightmap can write_buffer
        let normals_packed: Vec<u32> = normal_map
            .nx
            .iter()
            .zip(normal_map.ny.iter())
            .map(|(&nx, &ny)| {
                let xi = (nx.clamp(-1.0, 1.0) * 32767.0).round() as i16;
                let yi = (ny.clamp(-1.0, 1.0) * 32767.0).round() as i16;
                ((xi as u32) << 16) | (yi as u16 as u32)
            })
            .collect();
        let normals_packed_buf = vram::create_buffer_init_tracked(
            &gpu_ctx.device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("normals_packed"),
                contents: bytemuck::cast_slice(&normals_packed),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            },
            "normals_packed",
        );

        // shadow buffer (COPY_DST so update_shadow can write_buffer)
        let shadow_buf = vram::create_buffer_init_tracked(
            &gpu_ctx.device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("shadow"),
                contents: bytemuck::cast_slice(&shadow_mask.data),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            },
            "shadow",
        );

        // camera uniform (128 bytes, overwritten every frame)
        let cam_buf = vram::create_buffer_tracked(
            &gpu_ctx.device,
            &wgpu::BufferDescriptor {
                label: Some("cam"),
                size: std::mem::size_of::<CameraUniforms>() as u64,
                mapped_at_creation: false,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            },
            "cam",
        );

        // output buffer (fixed size, reused every frame)
        let output_buf = vram::create_buffer_tracked(
            &gpu_ctx.device,
            &wgpu::BufferDescriptor {
                label: Some("output"),
                size: (width * height * 4) as u64,
                mapped_at_creation: false,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            },
            "output",
        );

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
                        // ortho albedo windows (fine 20/21, close 22/23)
                        wgpu::BindGroupLayoutEntry {
                            binding: 20,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 21,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 22,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 23,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
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
                        resource: normals_packed_buf.as_entire_binding(),
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
                    // ortho albedo windows (placeholders)
                    wgpu::BindGroupEntry {
                        binding: 20,
                        resource: wgpu::BindingResource::TextureView(&ortho_fine_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 21,
                        resource: wgpu::BindingResource::Sampler(&ortho_fine_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 22,
                        resource: wgpu::BindingResource::TextureView(&ortho_close_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 23,
                        resource: wgpu::BindingResource::Sampler(&ortho_close_sampler),
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
            _normals_packed_buf: normals_packed_buf,
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
            hm5m_cos_rot: 1.0,
            hm5m_sin_rot: 0.0,
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
            hm1m_cos_rot: 1.0,
            hm1m_sin_rot: 0.0,
            hm1m_buf_elems: 1,
            _ortho_fine_tex: ortho_fine_tex,
            _ortho_fine_view: ortho_fine_view,
            _ortho_fine_sampler: ortho_fine_sampler,
            ortho_fine_origin_x: 0.0,
            ortho_fine_origin_y: 0.0,
            ortho_fine_extent_x: 0.0,
            ortho_fine_extent_y: 0.0,
            ortho_fine_cos_rot: 1.0,
            ortho_fine_sin_rot: 0.0,
            ortho_fine_cols: 0,
            ortho_fine_rows: 0,
            _ortho_close_tex: ortho_close_tex,
            _ortho_close_view: ortho_close_view,
            _ortho_close_sampler: ortho_close_sampler,
            ortho_close_origin_x: 0.0,
            ortho_close_origin_y: 0.0,
            ortho_close_extent_x: 0.0,
            ortho_close_extent_y: 0.0,
            ortho_close_cos_rot: 1.0,
            ortho_close_sin_rot: 0.0,
            ortho_close_cols: 0,
            ortho_close_rows: 0,
            shadow_buf,
            cam_buf,
            output_buf,
            render_pipeline,
            render_bg,
            render_bgl,
            width,
            height,
            hm_cols: hm.cols as u32,
            hm_rows: hm.rows as u32,
            dx_meters: hm.dx_meters as f32,
            dy_meters: hm.dy_meters as f32,
            max_terrain_h: hm.data.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
        }
    }

    /// Dispatches one frame. Only writes 128 bytes (camera uniform) then dispatches.
    // The args are precisely the *dynamic* (per-frame, input-driven) fields of the std140
    // `CameraUniforms` this method assembles — camera pose, sun, and render-mode flags that
    // change every frame from the viewer's input loop. The *static* scene state (heightmap
    // textures, tier geometry, pipelines) already lives in `self`. Bundling these into a
    // struct would just reconstruct a slice of `CameraUniforms` for the caller to fill — the
    // exact layout this method exists to build — so the flat list is kept deliberately.
    #[allow(clippy::too_many_arguments)]
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
        smooth_radius_m: f32,
        align_mode: u32,
        ortho_mode: u32,
    ) {
        let (forward, right, up) = crate::camera::camera_basis(origin, look_at);
        let (half_w, half_h) = crate::camera::projection_half_extents(fov_deg, aspect);

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
            hm5m_cos_rot: self.hm5m_cos_rot,
            hm5m_sin_rot: self.hm5m_sin_rot,
            hm1m_origin_x: self.hm1m_origin_x,
            hm1m_origin_y: self.hm1m_origin_y,
            hm1m_extent_x: self.hm1m_extent_x,
            hm1m_extent_y: self.hm1m_extent_y,
            hm1m_cols: self.hm1m_cols,
            hm1m_rows: self.hm1m_rows,
            hm1m_cos_rot: self.hm1m_cos_rot,
            hm1m_sin_rot: self.hm1m_sin_rot,
            max_terrain_h: self.max_terrain_h,
            smooth_radius_m,
            align_mode,
            _pad7: 0.0,
            ortho_fine_origin_x: self.ortho_fine_origin_x,
            ortho_fine_origin_y: self.ortho_fine_origin_y,
            ortho_fine_extent_x: self.ortho_fine_extent_x,
            ortho_fine_extent_y: self.ortho_fine_extent_y,
            ortho_fine_cos_rot: self.ortho_fine_cos_rot,
            ortho_fine_sin_rot: self.ortho_fine_sin_rot,
            ortho_close_origin_x: self.ortho_close_origin_x,
            ortho_close_origin_y: self.ortho_close_origin_y,
            ortho_close_extent_x: self.ortho_close_extent_x,
            ortho_close_extent_y: self.ortho_close_extent_y,
            ortho_close_cos_rot: self.ortho_close_cos_rot,
            ortho_close_sin_rot: self.ortho_close_sin_rot,
            ortho_mode,
            _pad8: 0.0,
            _pad9: 0.0,
            _pad10: 0.0,
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
            pass.dispatch_workgroups(self.width.div_ceil(8), self.height.div_ceil(8), 1);
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;

        vram::track_buffer_drop(&self.output_buf, "output");
        self.output_buf = vram::create_buffer_tracked(
            &self.gpu_ctx.device,
            &wgpu::BufferDescriptor {
                label: Some("output"),
                size: (width * height * 4) as u64,
                mapped_at_creation: false,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            },
            "output",
        );

        self.rebuild_bind_group();
    }

    /// Re-upload shadow mask (call when sun direction changes).
    pub fn update_ao(&self, ao_u8: &[u8]) {
        self.gpu_ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self._ao_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            ao_u8,
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
    /// When tile dimensions differ from the current GPU allocation, all size-dependent
    /// resources (hm texture, AO texture, normals buffer, shadow buffer) are recreated
    /// so that the shader UV formula `pos / (hm_cols * dx_meters)` stays correct.
    pub fn update_heightmap(
        &mut self,
        hm: &Heightmap,
        hm_f16: &[u8],
        hm_mips: &[(u32, u32, Vec<u8>)],
        normals_packed: &[u8],
        ao_u8: &[u8],
    ) {
        let new_cols = hm.cols as u32;
        let new_rows = hm.rows as u32;

        if new_cols != self.hm_cols || new_rows != self.hm_rows {
            // Drop-first cycle for the base tier: release the BindGroup's Arcs on
            // the old hm/ao/normals/shadow resources before allocating the new
            // ones. With the 10800×10800 Tirol demo grid the base tier alone is
            // ~1.3 GB; on a 4 GB GPU, holding the old plus the new is ~2.6 GB,
            // which combined with the close+fine tiers is what trips OOM. The
            // poll(Wait) drains wgpu's destroy-after-submission queue so the
            // free actually happens before the next allocation.
            //
            // The shader keeps sampling self._hm_texture during the swap window,
            // but reload events are dispatched between frames (update_heightmap is
            // called from the main loop in-between dispatch_frame calls), so no
            // concurrent compute pass observes the 1×1 placeholder.
            vram::track_texture_drop(&self._hm_texture, "scene_hm_tex");
            vram::track_texture_drop(&self._ao_texture, "scene_ao_tex");
            vram::track_buffer_drop(&self._normals_packed_buf, "normals_packed");
            vram::track_buffer_drop(&self.shadow_buf, "shadow");

            // 1×1 placeholders. Sample type matches the bind group layout
            // (Float filterable / Storage), so binding 1/4/7/8 stay valid.
            let ph_hm = vram::create_texture_tracked(
                &self.gpu_ctx.device,
                &wgpu::TextureDescriptor {
                    label: Some("scene_hm_tex"),
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
                },
                "scene_hm_tex",
            );
            self._hm_view = ph_hm.create_view(&wgpu::TextureViewDescriptor::default());
            self._hm_texture = ph_hm;

            let ph_ao = vram::create_texture_tracked(
                &self.gpu_ctx.device,
                &wgpu::TextureDescriptor {
                    label: Some("scene_ao_tex"),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                },
                "scene_ao_tex",
            );
            self._ao_view = ph_ao.create_view(&wgpu::TextureViewDescriptor::default());
            self._ao_texture = ph_ao;

            self._normals_packed_buf = vram::create_buffer_tracked(
                &self.gpu_ctx.device,
                &wgpu::BufferDescriptor {
                    label: Some("normals_packed"),
                    size: 4,
                    mapped_at_creation: false,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                },
                "normals_packed",
            );
            self.shadow_buf = vram::create_buffer_tracked(
                &self.gpu_ctx.device,
                &wgpu::BufferDescriptor {
                    label: Some("shadow"),
                    size: 4,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                },
                "shadow",
            );

            self.rebuild_bind_group();
            let _ = self.gpu_ctx.device.poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            });

            // Old resources are now actually freed on the GPU. Allocate the real new ones.
            vram::track_texture_drop(&self._hm_texture, "scene_hm_tex");
            self._hm_texture = vram::create_texture_tracked(
                &self.gpu_ctx.device,
                &wgpu::TextureDescriptor {
                    label: Some("scene_hm_tex"),
                    size: wgpu::Extent3d {
                        width: new_cols,
                        height: new_rows,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: crate::hm_mip_count(new_cols, new_rows),
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R16Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                },
                "scene_hm_tex",
            );
            self._hm_view = self
                ._hm_texture
                .create_view(&wgpu::TextureViewDescriptor::default());

            vram::track_texture_drop(&self._ao_texture, "scene_ao_tex");
            self._ao_texture = vram::create_texture_tracked(
                &self.gpu_ctx.device,
                &wgpu::TextureDescriptor {
                    label: Some("scene_ao_tex"),
                    size: wgpu::Extent3d {
                        width: new_cols,
                        height: new_rows,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                },
                "scene_ao_tex",
            );
            self._ao_view = self
                ._ao_texture
                .create_view(&wgpu::TextureViewDescriptor::default());

            vram::track_buffer_drop(&self._normals_packed_buf, "normals_packed");
            self._normals_packed_buf = vram::create_buffer_tracked(
                &self.gpu_ctx.device,
                &wgpu::BufferDescriptor {
                    label: Some("normals_packed"),
                    size: new_cols as u64 * new_rows as u64 * 4,
                    mapped_at_creation: false,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                },
                "normals_packed",
            );

            // No init: update_shadow is always called immediately after update_heightmap.
            vram::track_buffer_drop(&self.shadow_buf, "shadow");
            self.shadow_buf = vram::create_buffer_tracked(
                &self.gpu_ctx.device,
                &wgpu::BufferDescriptor {
                    label: Some("shadow"),
                    size: new_cols as u64 * new_rows as u64 * 4,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                },
                "shadow",
            );

            self.rebuild_bind_group();
        }

        let _t0 = std::time::Instant::now();
        self.gpu_ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self._hm_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            hm_f16,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(new_cols * 2),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: new_cols,
                height: new_rows,
                depth_or_array_layers: 1,
            },
        );
        eprintln!(
            "base write_texture hm_f16:    {:>6.1} ms ({new_cols}×{new_rows})",
            _t0.elapsed().as_secs_f32() * 1e3
        );
        let _t1 = std::time::Instant::now();
        write_hm_mips(&self.gpu_ctx.queue, &self._hm_texture, hm_mips);
        eprintln!(
            "base write_hm_mips:           {:>6.1} ms",
            _t1.elapsed().as_secs_f32() * 1e3
        );
        let _t2 = std::time::Instant::now();
        self.gpu_ctx
            .queue
            .write_buffer(&self._normals_packed_buf, 0, normals_packed);
        eprintln!(
            "base write_buffer normals:    {:>6.1} ms",
            _t2.elapsed().as_secs_f32() * 1e3
        );
        let _t3 = std::time::Instant::now();
        self.gpu_ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self._ao_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            ao_u8,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(new_cols),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: new_cols,
                height: new_rows,
                depth_or_array_layers: 1,
            },
        );
        eprintln!(
            "base write_texture ao_u8:     {:>6.1} ms",
            _t3.elapsed().as_secs_f32() * 1e3
        );

        self.hm_cols = new_cols;
        self.hm_rows = new_rows;
        self.dx_meters = hm.dx_meters as f32;
        self.dy_meters = hm.dy_meters as f32;
        self.max_terrain_h = hm.data.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
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
