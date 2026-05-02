use super::GpuScene;

impl GpuScene {
    /// Rebuild the stable group-0 bind group for the viewer pipeline (no binding 3).
    /// Called on construction and after any tier upload that changes texture/buffer handles.
    pub(super) fn rebuild_viewer_bind_group(&mut self) {
        let Some(vbgl) = &self.viewer_bgl else { return };
        self.viewer_bg = Some(
            self.gpu_ctx
                .device
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("viewer_bg"),
                    layout: vbgl,
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
                        // binding 3 absent — output is in group 1 (per-frame)
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: self._nx_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 5,
                            resource: self._ny_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 6,
                            resource: self._nz_buf.as_entire_binding(),
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
                            resource: wgpu::BindingResource::TextureView(
                                &self._hm5m_normal_view,
                            ),
                        },
                        wgpu::BindGroupEntry {
                            binding: 13,
                            resource: wgpu::BindingResource::Sampler(
                                &self._hm5m_normal_sampler,
                            ),
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
                            resource: wgpu::BindingResource::TextureView(
                                &self._hm1m_normal_view,
                            ),
                        },
                        wgpu::BindGroupEntry {
                            binding: 18,
                            resource: wgpu::BindingResource::Sampler(
                                &self._hm1m_normal_sampler,
                            ),
                        },
                        wgpu::BindGroupEntry {
                            binding: 19,
                            resource: self._hm1m_shadow_buf.as_entire_binding(),
                        },
                    ],
                }),
        );
    }

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
                        resource: self._nx_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: self._ny_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: self._nz_buf.as_entire_binding(),
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
        self.rebuild_viewer_bind_group();
    }
}
