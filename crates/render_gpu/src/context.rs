use wgpu::{Adapter, Instance};

pub struct GpuContext {
    pub instance: Instance,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter_name: String,
    pub adapter: Adapter,
}

impl GpuContext {
    pub fn new() -> Self {
        pollster::block_on(async {
            let instance = wgpu::Instance::default();

            // Enumerate all adapters and prefer discrete over integrated.
            let adapters: Vec<wgpu::Adapter> =
                instance.enumerate_adapters(wgpu::Backends::all()).await;
            for a in &adapters {
                let info = a.get_info();
                println!("  [GPU] found: {} ({:?})", info.name, info.device_type);
            }

            let adapter = if let Some(discrete) = adapters
                .into_iter()
                .find(|a| a.get_info().device_type == wgpu::DeviceType::DiscreteGpu)
            {
                discrete
            } else {
                instance
                    .request_adapter(&wgpu::RequestAdapterOptions {
                        power_preference: wgpu::PowerPreference::HighPerformance,
                        ..Default::default()
                    })
                    .await
                    .expect("no GPU adapter found")
            };

            let info = adapter.get_info();
            println!("  [GPU] selected: {} ({:?})", info.name, info.device_type);

            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .expect("failed to get device");

            GpuContext {
                instance,
                device,
                queue,
                adapter_name: info.name,
                adapter,
            }
        })
    }
}
