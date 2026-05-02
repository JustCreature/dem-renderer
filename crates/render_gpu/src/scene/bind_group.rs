use super::GpuScene;

impl GpuScene {
    pub(super) fn rebuild_bind_group(&mut self) {
        self.render_bg = self
            .gpu_ctx
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("render_bg"),
                layout: &self.render_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.cam_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&self._hm_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self._hm_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: self.output_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: self._normals_packed_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: self.shadow_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: wgpu::BindingResource::TextureView(&self._ao_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 9,
                        resource: wgpu::BindingResource::Sampler(&self._ao_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 10,
                        resource: wgpu::BindingResource::TextureView(&self._hm5m_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 11,
                        resource: wgpu::BindingResource::Sampler(&self._hm5m_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 12,
                        resource: wgpu::BindingResource::TextureView(&self._hm5m_normal_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 13,
                        resource: wgpu::BindingResource::Sampler(&self._hm5m_normal_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 14,
                        resource: self._hm5m_shadow_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 15,
                        resource: wgpu::BindingResource::TextureView(&self._hm1m_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 16,
                        resource: wgpu::BindingResource::Sampler(&self._hm1m_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 17,
                        resource: wgpu::BindingResource::TextureView(&self._hm1m_normal_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 18,
                        resource: wgpu::BindingResource::Sampler(&self._hm1m_normal_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 19,
                        resource: self._hm1m_shadow_buf.as_entire_binding(),
                    },
                ],
            });
    }
}
