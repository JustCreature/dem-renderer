mod camera;
mod context;
mod scene;
mod vector_utils;
pub mod vram;

pub use context::{
    GpuContext, OOM_COUNT, OOM_OBSERVED, VramClass, clear_oom_flag, signal_oom_for_testing,
};
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

/// Total mip level count we use for the base heightmap texture.
/// Clamped to whatever the texture size actually supports (wgpu rejects counts
/// greater than `floor(log2(max(W, H))) + 1`) and capped at 8 — the engine
/// convention for the base tier.
pub fn hm_mip_count(cols: u32, rows: u32) -> u32 {
    let max_dim = cols.max(rows).max(1);
    (max_dim.ilog2() + 1).min(8)
}

/// Generate mip level byte data (levels 1..N-1, where N = `hm_mip_count(cols, rows)`)
/// from a base R16Float byte buffer. Returns `(width, height, bytes)` per mip level.
/// Call on a background thread before `GpuScene::update_heightmap`.
pub fn gen_hm_mip_bytes(
    base_f16_bytes: &[u8],
    cols: usize,
    rows: usize,
) -> Vec<(u32, u32, Vec<u8>)> {
    let extra_mips = (hm_mip_count(cols as u32, rows as u32) as usize).saturating_sub(1);
    let mut mips: Vec<(u32, u32, Vec<u8>)> = Vec::with_capacity(extra_mips);
    let mut prev_w = cols;
    let mut prev_h = rows;
    for i in 0..extra_mips {
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

/// Generate mip level byte data for an RGBA8 ortho texture (levels 1..N-1,
/// N = `hm_mip_count(cols, rows)`). RGB channels are box-averaged; the alpha
/// channel — a discrete material-code ladder, not coverage — takes the top-left
/// sample instead, because averaging codes (0/64/128/192/255) would fabricate
/// classes that don't exist at that location.
/// Call on a background thread before `GpuScene::upload_ortho_*`.
pub fn gen_rgba_mip_bytes(base_rgba: &[u8], cols: usize, rows: usize) -> Vec<(u32, u32, Vec<u8>)> {
    let extra_mips = (hm_mip_count(cols as u32, rows as u32) as usize).saturating_sub(1);
    let mut mips: Vec<(u32, u32, Vec<u8>)> = Vec::with_capacity(extra_mips);
    let mut prev_w = cols;
    let mut prev_h = rows;
    for i in 0..extra_mips {
        let w = (prev_w / 2).max(1);
        let h = (prev_h / 2).max(1);
        let mip_bytes = {
            let prev: &[u8] = if i == 0 { base_rgba } else { &mips[i - 1].2 };
            let mut out = Vec::with_capacity(w * h * 4);
            for row in 0..h {
                for col in 0..w {
                    let r0 = (row * 2).min(prev_h - 1);
                    let r1 = (row * 2 + 1).min(prev_h - 1);
                    let c0 = (col * 2).min(prev_w - 1);
                    let c1 = (col * 2 + 1).min(prev_w - 1);
                    let px = |r: usize, c: usize| -> &[u8] {
                        let off = (r * prev_w + c) * 4;
                        &prev[off..off + 4]
                    };
                    let (p00, p01, p10, p11) = (px(r0, c0), px(r0, c1), px(r1, c0), px(r1, c1));
                    for ch in 0..3 {
                        let sum =
                            p00[ch] as u16 + p01[ch] as u16 + p10[ch] as u16 + p11[ch] as u16;
                        out.push((sum / 4) as u8);
                    }
                    out.push(p00[3]); // material code: nearest, never averaged
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

#[cfg(test)]
mod tests {
    //! Pure-CPU unit tests for the byte-packing / mip helpers. These need no GPU
    //! device and are the cheapest guard against this crate's recurring bug class:
    //! dimension / stride / mip-count assumptions that "always worked" for the
    //! data the author tested with (see the Diamond-Head `mip_level_count: 8`
    //! regression that `hm_mip_count` now fixes).

    use super::*;

    // hm_mip_count

    #[test]
    fn mip_count_diamond_head_regression() {
        // A 115×105 base-tier overview walked off the end of the old hardcoded
        // `mip_level_count: 8`: floor(log2(115)) + 1 = 7, and wgpu rejects a
        // count greater than what the texture supports.
        assert_eq!(hm_mip_count(115, 105), 7);
    }

    #[test]
    fn mip_count_caps_at_eight() {
        // floor(log2(8192)) + 1 = 14, but the engine convention caps the base
        // tier at 8 mips.
        assert_eq!(hm_mip_count(8192, 8192), 8);
    }

    #[test]
    fn mip_count_uses_longest_axis_and_min_one() {
        assert_eq!(hm_mip_count(1, 1), 1); // degenerate 1×1 → single level
        assert_eq!(hm_mip_count(128, 1), 8); // long axis drives it: log2(128)+1 = 8
        assert_eq!(hm_mip_count(1, 128), 8); // axis order doesn't matter
        assert_eq!(hm_mip_count(0, 0), 1); // .max(1) guards the all-zero case
    }

    // gen_hm_mip_bytes

    fn decode_f16_le(bytes: &[u8], idx: usize) -> f32 {
        half::f16::from_ne_bytes([bytes[idx * 2], bytes[idx * 2 + 1]]).to_f32()
    }

    // Byte-length asserts are written as `cols * rows * 2` (the f16 byte size) with the
    // mip's literal dimensions, so `1 * 1 * 2` stays readable instead of collapsing to `2`.
    #[allow(clippy::identity_op)]
    #[test]
    fn mip_pyramid_shapes_and_lengths() {
        // 4×4 base → hm_mip_count(4,4) = 3 → 2 generated levels: 2×2 then 1×1.
        let base: Vec<f32> = (0..16).map(|i| i as f32).collect();
        let base_bytes = hm_to_f16_bytes(&base);
        let mips = gen_hm_mip_bytes(&base_bytes, 4, 4);

        assert_eq!(mips.len(), hm_mip_count(4, 4) as usize - 1);
        assert_eq!((mips[0].0, mips[0].1), (2, 2));
        assert_eq!(mips[0].2.len(), 2 * 2 * 2, "f16 = 2 bytes/texel");
        assert_eq!((mips[1].0, mips[1].1), (1, 1));
        assert_eq!(mips[1].2.len(), 1 * 1 * 2);
    }

    #[test]
    fn mip_reduction_is_box_max() {
        // Row-major 4×4, integer values exactly representable in f16:
        //   0  1  2  3
        //   4  5  6  7
        //   8  9 10 11
        //  12 13 14 15
        // mip0 cell (0,0) covers base (0,0),(0,1),(1,0),(1,1) = {0,1,4,5} → max 5.
        // mip0 cell (1,1) covers {10,11,14,15} → max 15.
        let base: Vec<f32> = (0..16).map(|i| i as f32).collect();
        let base_bytes = hm_to_f16_bytes(&base);
        let mips = gen_hm_mip_bytes(&base_bytes, 4, 4);

        let m0 = &mips[0].2;
        assert_eq!(decode_f16_le(m0, 0), 5.0); // (0,0)
        assert_eq!(decode_f16_le(m0, 3), 15.0); // (1,1)

        // mip1 is the max of the whole image = 15.
        assert_eq!(decode_f16_le(&mips[1].2, 0), 15.0);
    }

    #[test]
    fn mip_dimensions_floor_with_min_one() {
        // 5×3 base: 5/2 = 2, 3/2 = 1 for the first generated level. The .max(1)
        // floors keep every dimension ≥ 1 as the pyramid shrinks.
        let base: Vec<f32> = vec![1.0; 15];
        let base_bytes = hm_to_f16_bytes(&base);
        let mips = gen_hm_mip_bytes(&base_bytes, 5, 3);
        assert_eq!((mips[0].0, mips[0].1), (2, 1));
        for (w, h, bytes) in &mips {
            assert!(*w >= 1 && *h >= 1);
            assert_eq!(bytes.len(), (*w * *h * 2) as usize);
        }
    }

    // gen_rgba_mip_bytes

    #[test]
    fn rgba_mip_shapes_match_hm_pyramid() {
        // 4×4 RGBA base → 2 generated levels (2×2, 1×1), 4 bytes/texel.
        let base = vec![100u8; 4 * 4 * 4];
        let mips = gen_rgba_mip_bytes(&base, 4, 4);
        assert_eq!(mips.len(), hm_mip_count(4, 4) as usize - 1);
        assert_eq!((mips[0].0, mips[0].1), (2, 2));
        assert_eq!(mips[0].2.len(), 2 * 2 * 4);
        assert_eq!((mips[1].0, mips[1].1), (1, 1));
        assert_eq!(mips[1].2.len(), 4);
    }

    // The expected value is written as `(0 + 40 + 80 + 120) / 4` so the four
    // contributing texels stay visible, instead of collapsing to a bare `60`.
    #[allow(clippy::identity_op)]
    #[test]
    fn rgba_mip_averages_rgb_but_takes_nearest_alpha() {
        // 2×2 base where the four texels have RGB 0/40/80/120 and alphas that
        // would average to a nonexistent material code (0+255+64+128)/4 = 111.
        #[rustfmt::skip]
        let base = vec![
            0, 0, 0, 0,        40, 40, 40, 255,
            80, 80, 80, 64,    120, 120, 120, 128,
        ];
        let mips = gen_rgba_mip_bytes(&base, 2, 2);
        assert_eq!((mips[0].0, mips[0].1), (1, 1));
        let m = &mips[0].2;
        assert_eq!(m[0], (0 + 40 + 80 + 120) / 4, "RGB box-averaged");
        assert_eq!(m[3], 0, "alpha = top-left sample, never an average");
    }

    #[test]
    fn rgba_mip_odd_dimensions_clamp_like_hm_path() {
        let base = vec![7u8; 5 * 3 * 4];
        let mips = gen_rgba_mip_bytes(&base, 5, 3);
        assert_eq!((mips[0].0, mips[0].1), (2, 1));
        for (w, h, bytes) in &mips {
            assert_eq!(bytes.len(), (*w * *h * 4) as usize);
        }
    }

    // normal packing

    fn decode_rg16(bytes: &[u8], idx: usize) -> (f32, f32) {
        let o = idx * 4;
        let xi = i16::from_ne_bytes([bytes[o], bytes[o + 1]]);
        let yi = i16::from_ne_bytes([bytes[o + 2], bytes[o + 3]]);
        (xi as f32 / 32767.0, yi as f32 / 32767.0)
    }

    #[test]
    fn rg16_round_trips_within_snorm_quantum() {
        let nx = [0.5, -0.25, 0.0, 1.0];
        let ny = [0.0, 1.0, -1.0, -0.5];
        let bytes = pack_normals_rg16_bytes(&nx, &ny);
        assert_eq!(bytes.len(), nx.len() * 4, "Rg16Snorm = 4 bytes/texel");

        let quantum = 1.0 / 32767.0;
        for i in 0..nx.len() {
            let (x, y) = decode_rg16(&bytes, i);
            assert!((x - nx[i]).abs() <= quantum, "x[{i}]: {x} vs {}", nx[i]);
            assert!((y - ny[i]).abs() <= quantum, "y[{i}]: {y} vs {}", ny[i]);
        }
    }

    #[test]
    fn rg16_clamps_out_of_range_to_full_scale() {
        let bytes = pack_normals_rg16_bytes(&[1.2, -1.2], &[-5.0, 5.0]);
        let xi0 = i16::from_ne_bytes([bytes[0], bytes[1]]);
        let yi0 = i16::from_ne_bytes([bytes[2], bytes[3]]);
        let xi1 = i16::from_ne_bytes([bytes[4], bytes[5]]);
        let yi1 = i16::from_ne_bytes([bytes[6], bytes[7]]);
        assert_eq!(xi0, 32767);
        assert_eq!(yi0, -32767);
        assert_eq!(xi1, -32767);
        assert_eq!(yi1, 32767);
    }

    #[test]
    fn u32_packing_layout_is_hi_x_lo_y() {
        // Same quantisation as rg16, packed as (xi << 16) | (yi as u16).
        let nx = [0.5, -0.25];
        let ny = [-0.5, 1.0];
        let bytes = pack_normals_u32_bytes(&nx, &ny);
        assert_eq!(bytes.len(), nx.len() * 4);

        for i in 0..nx.len() {
            let o = i * 4;
            let packed = u32::from_ne_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
            let hi = (packed >> 16) as i16; // x
            let lo = (packed & 0xffff) as i16; // y
            let expect_x = (nx[i].clamp(-1.0, 1.0) * 32767.0).round() as i16;
            let expect_y = (ny[i].clamp(-1.0, 1.0) * 32767.0).round() as i16;
            assert_eq!(hi, expect_x);
            assert_eq!(lo, expect_y);
        }
    }

    // AO + heightmap byte packing

    #[test]
    fn ao_u8_scales_and_saturates() {
        // In-range values scale by 255 with truncation; out-of-range inputs
        // saturate because Rust's float→int `as` cast is saturating.
        let out = pack_ao_u8(&[1.0, 0.0, 0.5, 2.0, -1.0]);
        assert_eq!(out, vec![255, 0, 127, 255, 0]);
    }

    #[test]
    fn hm_to_f16_bytes_length_and_round_trip() {
        let data = [0.0, 1.0, 100.5, -50.25];
        let bytes = hm_to_f16_bytes(&data);
        assert_eq!(bytes.len(), data.len() * 2);
        for (i, &v) in data.iter().enumerate() {
            let back = decode_f16_le(&bytes, i);
            // f16 has ~3 significant decimal digits; compare via the f16 round-trip.
            assert_eq!(back, half::f16::from_f32(v).to_f32());
        }
    }
}
