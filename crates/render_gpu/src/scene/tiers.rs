use wgpu::util::DeviceExt;

use super::GpuScene;

impl GpuScene {
    /// Upload 5m close-tier data and rebuild the bind group (texture view changed).
    /// origin_x/y are tile-local metres of the 5m window's top-left corner.
    pub fn upload_hm5m(
        &mut self,
        origin_x: f32,
        origin_y: f32,
        hm5m: &dem_io::Heightmap,
        normals: &terrain::NormalMap,
        shadow: &terrain::ShadowMask,
    ) {
        let hm_data: Vec<half::f16> =
            hm5m.data.iter().map(|&v| half::f16::from_f32(v)).collect();
        let texture = self.gpu_ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hm5m_tex"),
            size: wgpu::Extent3d {
                width: hm5m.cols as u32,
                height: hm5m.rows as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.gpu_ctx.queue.write_texture(
            texture.as_image_copy(),
            bytemuck::cast_slice(&hm_data),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(hm5m.cols as u32 * 2),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: hm5m.cols as u32,
                height: hm5m.rows as u32,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = self.gpu_ctx.device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let nx_buf = self
            .gpu_ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("hm5m_nx"),
                contents: bytemuck::cast_slice(&normals.nx),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            });
        let ny_buf = self
            .gpu_ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("hm5m_ny"),
                contents: bytemuck::cast_slice(&normals.ny),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            });
        let nz_buf = self
            .gpu_ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("hm5m_nz"),
                contents: bytemuck::cast_slice(&normals.nz),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            });
        let shadow_buf = self
            .gpu_ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("hm5m_shadow"),
                contents: bytemuck::cast_slice(&shadow.data),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            });

        self.hm5m_origin_x = origin_x;
        self.hm5m_origin_y = origin_y;
        self.hm5m_extent_x = hm5m.cols as f32 * hm5m.dx_meters as f32;
        self.hm5m_extent_y = hm5m.rows as f32 * hm5m.dy_meters as f32;
        self.hm5m_cols = hm5m.cols as u32;
        self.hm5m_rows = hm5m.rows as u32;
        self._hm5m_texture = texture;
        self._hm5m_view = view;
        self._hm5m_sampler = sampler;
        self._hm5m_nx_buf = nx_buf;
        self._hm5m_ny_buf = ny_buf;
        self._hm5m_nz_buf = nz_buf;
        self._hm5m_shadow_buf = shadow_buf;

        // Rebuild bind group — texture view changed
        self.rebuild_bind_group();
    }

    /// Disables the 5 m close-tier in the shader by zeroing hm5m_extent_x.
    /// The shader checks `uniforms.hm5m_extent_x > 0.0` before sampling the 5 m texture,
    /// so 0.0 is the sentinel for "no active close tier".
    /// Call this when the base heightmap swaps (tile-local offsets become stale) or when
    /// no valid 5 m window is available for the current camera position.
    pub fn set_hm5m_inactive(&mut self) {
        self.hm5m_extent_x = 0.0;
    }

    /// Upload 1m fine-tier data and rebuild the bind group (texture view changed).
    /// origin_x/y are tile-local metres of the 1m window's top-left corner.
    pub fn upload_hm1m(
        &mut self,
        origin_x: f32,
        origin_y: f32,
        hm1m: &dem_io::Heightmap,
        normals: &terrain::NormalMap,
        shadow: &terrain::ShadowMask,
    ) {
        let hm_data: Vec<half::f16> =
            hm1m.data.iter().map(|&v| half::f16::from_f32(v)).collect();
        let texture = self.gpu_ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hm1m_tex"),
            size: wgpu::Extent3d {
                width: hm1m.cols as u32,
                height: hm1m.rows as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.gpu_ctx.queue.write_texture(
            texture.as_image_copy(),
            bytemuck::cast_slice(&hm_data),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(hm1m.cols as u32 * 2),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: hm1m.cols as u32,
                height: hm1m.rows as u32,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = self.gpu_ctx.device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let nx_buf = self.gpu_ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("hm1m_nx"),
            contents: bytemuck::cast_slice(&normals.nx),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        let ny_buf = self.gpu_ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("hm1m_ny"),
            contents: bytemuck::cast_slice(&normals.ny),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        let nz_buf = self.gpu_ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("hm1m_nz"),
            contents: bytemuck::cast_slice(&normals.nz),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        let shadow_buf = self.gpu_ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("hm1m_shadow"),
            contents: bytemuck::cast_slice(&shadow.data),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        self.hm1m_origin_x = origin_x;
        self.hm1m_origin_y = origin_y;
        self.hm1m_extent_x = hm1m.cols as f32 * hm1m.dx_meters as f32;
        self.hm1m_extent_y = hm1m.rows as f32 * hm1m.dy_meters as f32;
        self.hm1m_cols = hm1m.cols as u32;
        self.hm1m_rows = hm1m.rows as u32;
        self._hm1m_texture = texture;
        self._hm1m_view = view;
        self._hm1m_sampler = sampler;
        self._hm1m_nx_buf = nx_buf;
        self._hm1m_ny_buf = ny_buf;
        self._hm1m_nz_buf = nz_buf;
        self._hm1m_shadow_buf = shadow_buf;

        // Rebuild bind group — texture view changed
        self.rebuild_bind_group();
    }

    /// Disables the 1m fine-tier by zeroing hm1m_extent_x.
    pub fn set_hm1m_inactive(&mut self) {
        self.hm1m_extent_x = 0.0;
    }
}
