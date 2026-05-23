use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use wgpu::{Adapter, Instance};

/// Set by `device.on_uncaptured_error` when wgpu reports `OutOfMemory`. The
/// viewer's frame loop polls this each tick and downgrades the active tier
/// preset (fine first, close second) instead of letting wgpu panic.
///
/// Counts up-edges so the viewer can distinguish "one OOM happened" from "two
/// OOMs happened" between polls — important because a single failed allocation
/// may produce multiple OOM events as wgpu retries on different heaps.
pub static OOM_OBSERVED: AtomicBool = AtomicBool::new(false);
pub static OOM_COUNT: AtomicU32 = AtomicU32::new(0);

/// Test-only entry point: simulate an OOM signal from the debug hotkey.
/// Identical to what `device.on_uncaptured_error` does on a real OOM.
pub fn signal_oom_for_testing() {
    OOM_OBSERVED.store(true, Ordering::SeqCst);
    OOM_COUNT.fetch_add(1, Ordering::SeqCst);
}

/// Reset both OOM atomics. Called by the viewer after it has reacted to a
/// degradation event so the next OOM can be distinguished from the prior one.
pub fn clear_oom_flag() {
    OOM_OBSERVED.store(false, Ordering::SeqCst);
}

/// Coarse VRAM budget class used to drive tier-radius scaling.
///
/// Detected from `adapter.get_info()` because wgpu 0.29 does not expose any
/// memory-size query. The default mapping is conservative — only known-tiny
/// cards are tagged `Low`; everything else gets `Mid` (or `High` for Apple
/// Silicon, where unified memory removes the constraint).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VramClass {
    /// Roughly < 4 GB usable. Halves tier radii and disables the fine tier.
    Low,
    /// 4–8 GB usable, or unknown discrete. Modest tier-radius reduction.
    Mid,
    /// 8 GB+, or Apple Silicon unified memory. Full radii.
    High,
}

impl VramClass {
    /// Heuristic: name-substring match for known low-VRAM cards, then a few
    /// device-type fallbacks. Adapter names are normalised to lowercase before
    /// matching. Conservative on purpose — false negatives (a tiny card
    /// classified as Mid) surface as OOM at runtime where the OOM handler
    /// downgrades them; false positives (a healthy card classified as Low)
    /// silently rob the user of detail and are harder to detect.
    pub fn detect(info: &wgpu::AdapterInfo) -> Self {
        let name = info.name.to_lowercase();

        // Apple Silicon — unified memory; the M-series with 16 GB+ has zero
        // budget pressure for this app's scale. Detect before the integrated
        // fallback so M-series isn't mis-tagged.
        if name.contains("apple m") {
            return VramClass::High;
        }

        // Known-tiny GPUs that the Mid preset would push over the edge —
        // roughly ≤ 2 GB dedicated VRAM in their common configurations. Cards
        // with 3 GB+ (GTX 1050 / 1650 / 1660 and friends) handle Mid fine; the
        // runtime OOM handler catches them on the rare reload spike. Match by
        // substring to catch model suffixes ("super", "ti", "mobile", etc.).
        let low_vram_substrings = [
            // Nvidia laptop dGPUs (2 GB).
            "mx150",
            "mx250",
            "mx350",
            "mx450",
            // Older / weaker integrated graphics with tiny reserved budgets.
            "hd graphics 4",
            "hd graphics 5",
            "uhd graphics 6",
            // Entry-level AMD discrete (2 GB).
            "rx 550",
            "rx 560",
            // Intel Arc entry parts.
            "arc a310",
            "arc a380",
        ];
        if low_vram_substrings.iter().any(|s| name.contains(s)) {
            return VramClass::Low;
        }

        // Default by device type. IntegratedGpu is NOT Low by default — many
        // modern integrated GPUs (Iris Plus, Vega 8, M-series) ship with
        // enough shared memory to handle the full demo; only the truly tiny
        // ones above are explicitly downgraded.
        match info.device_type {
            wgpu::DeviceType::Cpu | wgpu::DeviceType::Other => VramClass::Low,
            wgpu::DeviceType::IntegratedGpu
            | wgpu::DeviceType::DiscreteGpu
            | wgpu::DeviceType::VirtualGpu => VramClass::Mid,
        }
    }
}

// wgpu::Device and wgpu::Queue are internally Arc-backed.
// Cloning one does not create a second GPU connection — it hands you a second Arc
// pointer to the same underlying device.
// So gpu_ctx.clone() is a cheap reference count bump, not a GPU re-init.
#[derive(Clone)]
pub struct GpuContext {
    pub instance: Instance,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter_name: String,
    pub adapter: Adapter,
    /// Detected VRAM budget class. Drives the default tier-radius preset; a
    /// later launcher UI override can override this without re-creating the
    /// device.
    pub vram_class: VramClass,
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
            let vram_class = VramClass::detect(&info);
            println!(
                "  [GPU] selected: {} ({:?})  vram_class={:?}",
                info.name, info.device_type, vram_class
            );

            // Request optional features that improve precision; only enable what the
            // adapter actually supports so the build stays cross-platform.
            let wanted = wgpu::Features::FLOAT32_FILTERABLE        // R32Float + Linear sampler
                       | wgpu::Features::TEXTURE_FORMAT_16BIT_NORM; // Rg16Snorm normal textures
            let enabled = adapter.features() & wanted;

            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    required_features: enabled,
                    required_limits: adapter.limits(),
                    ..Default::default()
                })
                .await
                .expect("failed to get device");

            // OOM safety net: wgpu's default behaviour on a failed allocation is
            // to panic from a worker thread, which kills the whole process. The
            // handler here logs the error and sets a flag that the viewer's
            // frame loop polls to degrade gracefully (disable fine tier, then
            // close tier) instead of crashing. Runs on wgpu's internal thread —
            // no blocking, no allocation, just an atomic store.
            device.on_uncaptured_error(Arc::new(|err: wgpu::Error| {
                eprintln!("[GPU ERROR] {err:?}");
                if matches!(err, wgpu::Error::OutOfMemory { .. }) {
                    OOM_OBSERVED.store(true, Ordering::SeqCst);
                    OOM_COUNT.fetch_add(1, Ordering::SeqCst);
                }
            }));

            GpuContext {
                instance,
                device,
                queue,
                adapter_name: info.name,
                adapter,
                vram_class,
            }
        })
    }
}
