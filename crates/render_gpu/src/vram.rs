//! GPU allocation accounting for the dem-renderer scene.
//!
//! wgpu doesn't expose VRAM residency, so we mirror what we ask the device to
//! create. Allocations are added to the counter at `create_*` time; drops are
//! tracked explicitly at the call sites that replace a stored field (where the
//! BindGroup releases its Arc on the next rebuild). The numbers approximate
//! CPU-side claimed bytes — useful for comparing reload peaks before and after
//! the eager-dealloc fix.

use std::sync::atomic::{AtomicU64, Ordering};

use wgpu::util::DeviceExt;

pub static GPU_TEXTURE_BYTES: AtomicU64 = AtomicU64::new(0);
pub static GPU_BUFFER_BYTES: AtomicU64 = AtomicU64::new(0);

fn format_bpp(fmt: wgpu::TextureFormat) -> u64 {
    match fmt {
        wgpu::TextureFormat::R8Unorm | wgpu::TextureFormat::R8Snorm => 1,
        wgpu::TextureFormat::R16Float
        | wgpu::TextureFormat::R16Snorm
        | wgpu::TextureFormat::R16Unorm => 2,
        wgpu::TextureFormat::R32Float
        | wgpu::TextureFormat::Rg16Float
        | wgpu::TextureFormat::Rg16Snorm
        | wgpu::TextureFormat::Rg16Unorm
        | wgpu::TextureFormat::Rgba8Unorm
        | wgpu::TextureFormat::Rgba8Snorm
        | wgpu::TextureFormat::Rgba8UnormSrgb
        | wgpu::TextureFormat::Bgra8Unorm
        | wgpu::TextureFormat::Bgra8UnormSrgb => 4,
        wgpu::TextureFormat::Rg32Float | wgpu::TextureFormat::Rgba16Float => 8,
        wgpu::TextureFormat::Rgba32Float => 16,
        _ => 4,
    }
}

fn texture_bytes_from_desc(desc: &wgpu::TextureDescriptor<'_>) -> u64 {
    let bpp = format_bpp(desc.format);
    let d = desc.size.depth_or_array_layers as u64;
    let mut total = 0u64;
    for level in 0..desc.mip_level_count {
        let w = (desc.size.width >> level).max(1) as u64;
        let h = (desc.size.height >> level).max(1) as u64;
        total += w * h * d * bpp;
    }
    total
}

fn texture_bytes_of(t: &wgpu::Texture) -> u64 {
    let size = t.size();
    let bpp = format_bpp(t.format());
    let d = size.depth_or_array_layers as u64;
    let mut total = 0u64;
    for level in 0..t.mip_level_count() {
        let w = (size.width >> level).max(1) as u64;
        let h = (size.height >> level).max(1) as u64;
        total += w * h * d * bpp;
    }
    total
}

fn log_event(kind: &str, label: &str, sign: char, bytes: u64) {
    let tex_mb = GPU_TEXTURE_BYTES.load(Ordering::Relaxed) as f64 / (1024.0 * 1024.0);
    let buf_mb = GPU_BUFFER_BYTES.load(Ordering::Relaxed) as f64 / (1024.0 * 1024.0);
    eprintln!(
        "[vram] {kind:>9} {label:<22} {sign}{:>7.2} MB  (tex {tex_mb:>7.1} MB, buf {buf_mb:>7.1} MB)",
        bytes as f64 / (1024.0 * 1024.0),
    );
}

pub fn create_texture_tracked(
    device: &wgpu::Device,
    desc: &wgpu::TextureDescriptor<'_>,
    log_label: &str,
) -> wgpu::Texture {
    let bytes = texture_bytes_from_desc(desc);
    GPU_TEXTURE_BYTES.fetch_add(bytes, Ordering::Relaxed);
    log_event("alloc tex", log_label, '+', bytes);
    device.create_texture(desc)
}

pub fn create_buffer_tracked(
    device: &wgpu::Device,
    desc: &wgpu::BufferDescriptor<'_>,
    log_label: &str,
) -> wgpu::Buffer {
    let bytes = desc.size;
    GPU_BUFFER_BYTES.fetch_add(bytes, Ordering::Relaxed);
    log_event("alloc buf", log_label, '+', bytes);
    device.create_buffer(desc)
}

pub fn create_buffer_init_tracked(
    device: &wgpu::Device,
    desc: &wgpu::util::BufferInitDescriptor<'_>,
    log_label: &str,
) -> wgpu::Buffer {
    let bytes = desc.contents.len() as u64;
    GPU_BUFFER_BYTES.fetch_add(bytes, Ordering::Relaxed);
    log_event("alloc buf", log_label, '+', bytes);
    device.create_buffer_init(desc)
}

/// Account for an about-to-be-dropped texture. Call right before the Rust
/// handle is overwritten / replaced so the counter mirrors when the BindGroup
/// rebuild will let wgpu reclaim the resource.
pub fn track_texture_drop(t: &wgpu::Texture, log_label: &str) {
    let bytes = texture_bytes_of(t);
    GPU_TEXTURE_BYTES.fetch_sub(bytes, Ordering::Relaxed);
    log_event("drop tex", log_label, '-', bytes);
}

pub fn track_buffer_drop(b: &wgpu::Buffer, log_label: &str) {
    let bytes = b.size();
    GPU_BUFFER_BYTES.fetch_sub(bytes, Ordering::Relaxed);
    log_event("drop buf", log_label, '-', bytes);
}
