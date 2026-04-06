pub struct GpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl GpuContext {
    pub fn new() -> Self {
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

            GpuContext { device, queue }
        })
    }
}
