use super::GpuScene;

impl GpuScene {
    /// Upload 5m close-tier data.
    /// Grow-only GPU resources: texture and buffers are recreated only when the incoming
    /// window is larger than what is currently allocated; otherwise data is written in-place
    /// via write_texture / write_buffer, avoiding GPU memory allocation on steady-state reloads.
    pub fn upload_hm5m(
        &mut self,
        origin_x: f32,
        origin_y: f32,
        rot_rad: f32,
        extent_x: f32,
        extent_y: f32,
        hm5m: &dem_io::Heightmap,
        normals_rg16: &[u8],
        shadow: &terrain::ShadowMask,
    ) {
        let cols = hm5m.cols as u32;
        let rows = hm5m.rows as u32;
        let needed_elems = cols as u64 * rows as u64;

        let size_changed = cols != self.hm5m_cols || rows != self.hm5m_rows;
        let buf_too_small = needed_elems > self.hm5m_buf_elems;

        if size_changed {
            let texture = self
                .gpu_ctx
                .device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("hm5m_tex"),
                    size: wgpu::Extent3d {
                        width: cols,
                        height: rows,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R32Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
            self._hm5m_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self._hm5m_texture = texture;
            let normal_tex = self
                .gpu_ctx
                .device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("hm5m_normal_tex"),
                    size: wgpu::Extent3d {
                        width: cols,
                        height: rows,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rg16Snorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
            self._hm5m_normal_view =
                normal_tex.create_view(&wgpu::TextureViewDescriptor::default());
            self._hm5m_normal_tex = normal_tex;
        }

        if buf_too_small {
            let size = needed_elems * 4;
            self._hm5m_shadow_buf = self.gpu_ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("hm5m_shadow"),
                size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.hm5m_buf_elems = needed_elems;
        }

        if size_changed || buf_too_small {
            self.rebuild_bind_group();
        }

        let _t0 = std::time::Instant::now();
        self.gpu_ctx.queue.write_texture(
            self._hm5m_texture.as_image_copy(),
            bytemuck::cast_slice(&hm5m.data),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(cols * 4),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: cols,
                height: rows,
                depth_or_array_layers: 1,
            },
        );
        eprintln!(
            "5m write_texture hm:      {:>6.1} ms",
            _t0.elapsed().as_secs_f32() * 1e3
        );
        let _t1 = std::time::Instant::now();
        self.gpu_ctx.queue.write_texture(
            self._hm5m_normal_tex.as_image_copy(),
            normals_rg16,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(cols * 4),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: cols,
                height: rows,
                depth_or_array_layers: 1,
            },
        );
        eprintln!(
            "5m write_texture normals: {:>6.1} ms",
            _t1.elapsed().as_secs_f32() * 1e3
        );
        let _t2 = std::time::Instant::now();
        self.gpu_ctx.queue.write_buffer(
            &self._hm5m_shadow_buf,
            0,
            bytemuck::cast_slice(&shadow.data),
        );
        eprintln!(
            "5m write_buffer  shadow:  {:>6.1} ms",
            _t2.elapsed().as_secs_f32() * 1e3
        );

        self.hm5m_origin_x = origin_x;
        self.hm5m_origin_y = origin_y;
        self.hm5m_extent_x = extent_x;
        self.hm5m_extent_y = extent_y;
        self.hm5m_cols = cols;
        self.hm5m_rows = rows;
        self.hm5m_cos_rot = rot_rad.cos();
        self.hm5m_sin_rot = rot_rad.sin();
    }

    /// Disables the 5 m close-tier in the shader by zeroing hm5m_extent_x.
    pub fn set_hm5m_inactive(&mut self) {
        self.hm5m_extent_x = 0.0;
    }

    /// Upload 1m fine-tier data.
    /// Same grow-only strategy as upload_hm5m.
    pub fn upload_hm1m(
        &mut self,
        origin_x: f32,
        origin_y: f32,
        rot_rad: f32,
        extent_x: f32,
        extent_y: f32,
        hm1m: &dem_io::Heightmap,
        normals_rg16: &[u8],
        shadow: &terrain::ShadowMask,
    ) {
        let cols = hm1m.cols as u32;
        let rows = hm1m.rows as u32;
        let needed_elems = cols as u64 * rows as u64;

        let size_changed = cols != self.hm1m_cols || rows != self.hm1m_rows;
        let buf_too_small = needed_elems > self.hm1m_buf_elems;

        if size_changed {
            let texture = self
                .gpu_ctx
                .device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("hm1m_tex"),
                    size: wgpu::Extent3d {
                        width: cols,
                        height: rows,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R32Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
            self._hm1m_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self._hm1m_texture = texture;
            let normal_tex = self
                .gpu_ctx
                .device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("hm1m_normal_tex"),
                    size: wgpu::Extent3d {
                        width: cols,
                        height: rows,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rg16Snorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
            self._hm1m_normal_view =
                normal_tex.create_view(&wgpu::TextureViewDescriptor::default());
            self._hm1m_normal_tex = normal_tex;
        }

        if buf_too_small {
            let size = needed_elems * 4;
            self._hm1m_shadow_buf = self.gpu_ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("hm1m_shadow"),
                size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.hm1m_buf_elems = needed_elems;
        }

        if size_changed || buf_too_small {
            self.rebuild_bind_group();
        }

        self.gpu_ctx.queue.write_texture(
            self._hm1m_texture.as_image_copy(),
            bytemuck::cast_slice(&hm1m.data),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(cols * 4),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: cols,
                height: rows,
                depth_or_array_layers: 1,
            },
        );
        self.gpu_ctx.queue.write_texture(
            self._hm1m_normal_tex.as_image_copy(),
            normals_rg16,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(cols * 4),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: cols,
                height: rows,
                depth_or_array_layers: 1,
            },
        );
        self.gpu_ctx.queue.write_buffer(
            &self._hm1m_shadow_buf,
            0,
            bytemuck::cast_slice(&shadow.data),
        );

        self.hm1m_origin_x = origin_x;
        self.hm1m_origin_y = origin_y;
        self.hm1m_extent_x = extent_x;
        self.hm1m_extent_y = extent_y;
        self.hm1m_cols = cols;
        self.hm1m_rows = rows;
        self.hm1m_cos_rot = rot_rad.cos();
        self.hm1m_sin_rot = rot_rad.sin();
    }

    /// Disables the 1m fine-tier by zeroing hm1m_extent_x.
    pub fn set_hm1m_inactive(&mut self) {
        self.hm1m_extent_x = 0.0;
    }
}
