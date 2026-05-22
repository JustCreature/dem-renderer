mod camera;
mod context;
mod render_rexture;
mod scene;
mod vector_utils;
pub mod vram;

pub use context::GpuContext;
pub use render_rexture::render_gpu_texture;
pub use scene::GpuScene;

/// Convert f32 heightmap slice to native-endian f16 bytes for R16Float GPU texture upload.
/// Call on a background thread before `GpuScene::update_heightmap`.
pub fn hm_to_f16_bytes(data: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() * 2);
    for &v in data {
        out.extend_from_slice(&half::f16::from_f32(v).to_ne_bytes());
    }
    out
}

/// Generate mip level byte data (levels 1–7) from a base R16Float byte buffer.
/// Returns `(width, height, bytes)` per mip level.
/// Call on a background thread before `GpuScene::update_heightmap`.
pub fn gen_hm_mip_bytes(
    base_f16_bytes: &[u8],
    cols: usize,
    rows: usize,
) -> Vec<(u32, u32, Vec<u8>)> {
    let mut mips: Vec<(u32, u32, Vec<u8>)> = Vec::with_capacity(7);
    let mut prev_w = cols;
    let mut prev_h = rows;
    for i in 0..7_usize {
        let w = (prev_w / 2).max(1);
        let h = (prev_h / 2).max(1);
        let mip_bytes = {
            let prev: &[u8] = if i == 0 {
                base_f16_bytes
            } else {
                &mips[i - 1].2
            };
            let mut out = Vec::with_capacity(w * h * 2);
            for row in 0..h {
                for col in 0..w {
                    let r0 = (row * 2).min(prev_h - 1);
                    let r1 = (row * 2 + 1).min(prev_h - 1);
                    let c0 = (col * 2).min(prev_w - 1);
                    let c1 = (col * 2 + 1).min(prev_w - 1);
                    let get = |r: usize, c: usize| -> f32 {
                        let off = (r * prev_w + c) * 2;
                        half::f16::from_ne_bytes([prev[off], prev[off + 1]]).to_f32()
                    };
                    let v = get(r0, c0)
                        .max(get(r0, c1))
                        .max(get(r1, c0))
                        .max(get(r1, c1));
                    out.extend_from_slice(&half::f16::from_f32(v).to_ne_bytes());
                }
            }
            out
        };
        prev_w = w;
        prev_h = h;
        mips.push((w as u32, h as u32, mip_bytes));
    }
    mips
}

/// Pack normal vectors into Rg16Snorm bytes (4 bytes/pixel) for GPU texture upload.
/// Call on a background thread before `GpuScene::upload_hm5m` / `upload_hm1m`.
pub fn pack_normals_rg16_bytes(nx: &[f32], ny: &[f32]) -> Vec<u8> {
    debug_assert_eq!(nx.len(), ny.len());
    let mut out = Vec::with_capacity(nx.len() * 4);
    for (&nx_v, &ny_v) in nx.iter().zip(ny.iter()) {
        let xi = (nx_v.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        let yi = (ny_v.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        out.extend_from_slice(&xi.to_ne_bytes());
        out.extend_from_slice(&yi.to_ne_bytes());
    }
    out
}

/// Pack normal vectors into u32 storage-buffer bytes (4 bytes/pixel).
/// Call on a background thread before `GpuScene::update_heightmap`.
pub fn pack_normals_u32_bytes(nx: &[f32], ny: &[f32]) -> Vec<u8> {
    debug_assert_eq!(nx.len(), ny.len());
    let mut out = Vec::with_capacity(nx.len() * 4);
    for (&nx_v, &ny_v) in nx.iter().zip(ny.iter()) {
        let xi = (nx_v.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        let yi = (ny_v.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        let packed: u32 = ((xi as u32) << 16) | (yi as u16 as u32);
        out.extend_from_slice(&packed.to_ne_bytes());
    }
    out
}

/// Convert f32 AO values to u8 bytes for R8Unorm GPU texture upload.
/// Call on a background thread before `GpuScene::update_ao` / `update_heightmap`.
pub fn pack_ao_u8(ao: &[f32]) -> Vec<u8> {
    ao.iter().map(|&v| (v * 255.0) as u8).collect()
}
