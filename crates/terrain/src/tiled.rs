use crate::{NormalMap, SendPtr};
use rayon::prelude::*;

#[cfg(target_arch = "aarch64")]
pub unsafe fn compute_normals_neon_tiled(hm: &dem_io::TiledHeightmap) -> NormalMap {
    use core::arch::aarch64::*;

    let mut nx: Vec<f32> = vec![0.0f32; hm.rows * hm.cols];
    let mut ny: Vec<f32> = vec![0.0f32; hm.rows * hm.cols];
    let mut nz: Vec<f32> = vec![0.0f32; hm.rows * hm.cols];

    let inv_2dx_f32 = 1.0f32 / (2.0 * hm.dx_meters as f32);
    let inv_2dy_f32 = 1.0f32 / (2.0 * hm.dy_meters as f32);

    unsafe {
        let inv_2dx = vdupq_n_f32(inv_2dx_f32);
        let inv_2dy = vdupq_n_f32(inv_2dy_f32);

        for tr in 0..hm.tile_rows {
            for tc in 0..hm.tile_cols {
                let tile_start = (tr * hm.tile_cols + tc) * hm.tile_size * hm.tile_size;
                let tile_ptr = hm.tiles().as_ptr().add(tile_start);

                for local_r in 1..hm.tile_size - 1 {
                    let global_r = tr * hm.tile_size + local_r;
                    if global_r >= hm.rows - 1 {
                        continue;
                    }

                    let mut local_c = 1usize;

                    // NEON: 4 pixels at a time using tile_ptr with stride tile_size
                    while local_c + 4 < hm.tile_size - 1 {
                        let global_c = tc * hm.tile_size + local_c;
                        if global_c + 4 >= hm.cols {
                            break;
                        }

                        let ptr_upper = tile_ptr.add((local_r - 1) * hm.tile_size + local_c);
                        let upper = vcvtq_f32_s32(vmovl_s16(vld1_s16(ptr_upper)));

                        let ptr_lower = tile_ptr.add((local_r + 1) * hm.tile_size + local_c);
                        let lower = vcvtq_f32_s32(vmovl_s16(vld1_s16(ptr_lower)));

                        let ptr_left = tile_ptr.add(local_r * hm.tile_size + (local_c - 1));
                        let left = vcvtq_f32_s32(vmovl_s16(vld1_s16(ptr_left)));

                        let ptr_right = tile_ptr.add(local_r * hm.tile_size + (local_c + 1));
                        let right = vcvtq_f32_s32(vmovl_s16(vld1_s16(ptr_right)));

                        let vec_nx = vmulq_f32(vsubq_f32(left, right), inv_2dx);
                        let vec_ny = vmulq_f32(vsubq_f32(upper, lower), inv_2dy);
                        let vec_nz = vdupq_n_f32(1.0);

                        let len_sq = vaddq_f32(
                            vaddq_f32(vmulq_f32(vec_nx, vec_nx), vmulq_f32(vec_ny, vec_ny)),
                            vmulq_f32(vec_nz, vec_nz),
                        );
                        let est = vrsqrteq_f32(len_sq);
                        let refined = vmulq_f32(vrsqrtsq_f32(vmulq_f32(len_sq, est), est), est);

                        let out = global_r * hm.cols + global_c;
                        vst1q_f32(nx.as_mut_ptr().add(out), vmulq_f32(vec_nx, refined));
                        vst1q_f32(ny.as_mut_ptr().add(out), vmulq_f32(vec_ny, refined));
                        vst1q_f32(nz.as_mut_ptr().add(out), vmulq_f32(vec_nz, refined));

                        local_c += 4;
                    }

                    // scalar tail
                    while local_c < hm.tile_size - 1 {
                        let global_c = tc * hm.tile_size + local_c;
                        if global_c == 0 || global_c >= hm.cols - 1 {
                            local_c += 1;
                            continue;
                        }

                        let upper = *tile_ptr.add((local_r - 1) * hm.tile_size + local_c) as f32;
                        let lower = *tile_ptr.add((local_r + 1) * hm.tile_size + local_c) as f32;
                        let left = *tile_ptr.add(local_r * hm.tile_size + (local_c - 1)) as f32;
                        let right = *tile_ptr.add(local_r * hm.tile_size + (local_c + 1)) as f32;

                        let single_nx = (left - right) * inv_2dx_f32;
                        let single_ny = (upper - lower) * inv_2dy_f32;
                        let single_nz = 1.0f32;

                        let length = f32::sqrt(
                            single_nx * single_nx + single_ny * single_ny + single_nz * single_nz,
                        );

                        let out = global_r * hm.cols + global_c;
                        nx[out] = single_nx / length;
                        ny[out] = single_ny / length;
                        nz[out] = single_nz / length;

                        local_c += 1;
                    }
                }
            }
        }
    }

    NormalMap {
        nx,
        ny,
        nz,
        rows: hm.rows,
        cols: hm.cols,
    }
}

#[cfg(target_arch = "aarch64")]
pub unsafe fn compute_normals_neon_tiled_parallel(hm: &dem_io::TiledHeightmap) -> NormalMap {
    use core::arch::aarch64::*;

    let mut nx: Vec<f32> = vec![0.0f32; hm.rows * hm.cols];
    let mut ny: Vec<f32> = vec![0.0f32; hm.rows * hm.cols];
    let mut nz: Vec<f32> = vec![0.0f32; hm.rows * hm.cols];

    let nx_ptr = SendPtr(nx.as_mut_ptr());
    let ny_ptr = SendPtr(ny.as_mut_ptr());
    let nz_ptr = SendPtr(nz.as_mut_ptr());

    let inv_2dx_f32 = 1.0f32 / (2.0 * hm.dx_meters as f32);
    let inv_2dy_f32 = 1.0f32 / (2.0 * hm.dy_meters as f32);

    let tile_rows = hm.tile_rows;
    let tile_cols = hm.tile_cols;
    let tile_size = hm.tile_size;
    let rows = hm.rows;
    let cols = hm.cols;

    // Safety: each tile writes to a non-overlapping region of the output arrays.
    // Tile (tr, tc) writes to global rows [tr*tile_size, (tr+1)*tile_size) and
    // global cols [tc*tile_size, (tc+1)*tile_size). No two tiles share an output index.
    (0..tile_rows * tile_cols)
        .into_par_iter()
        .for_each(move |tile_idx| {
            let tr = tile_idx / tile_cols;
            let tc = tile_idx % tile_cols;

            unsafe {
                let inv_2dx = vdupq_n_f32(inv_2dx_f32);
                let inv_2dy = vdupq_n_f32(inv_2dy_f32);

                let tile_start = (tr * tile_cols + tc) * tile_size * tile_size;
                let tile_ptr = hm.tiles().as_ptr().add(tile_start);

                for local_r in 1..tile_size - 1 {
                    let global_r = tr * tile_size + local_r;
                    if global_r >= rows - 1 {
                        continue;
                    }

                    let mut local_c = 1usize;

                    while local_c + 4 < tile_size - 1 {
                        let global_c = tc * tile_size + local_c;
                        if global_c + 4 >= cols {
                            break;
                        }

                        let ptr_upper = tile_ptr.add((local_r - 1) * tile_size + local_c);
                        let upper = vcvtq_f32_s32(vmovl_s16(vld1_s16(ptr_upper)));

                        let ptr_lower = tile_ptr.add((local_r + 1) * tile_size + local_c);
                        let lower = vcvtq_f32_s32(vmovl_s16(vld1_s16(ptr_lower)));

                        let ptr_left = tile_ptr.add(local_r * tile_size + (local_c - 1));
                        let left = vcvtq_f32_s32(vmovl_s16(vld1_s16(ptr_left)));

                        let ptr_right = tile_ptr.add(local_r * tile_size + (local_c + 1));
                        let right = vcvtq_f32_s32(vmovl_s16(vld1_s16(ptr_right)));

                        let vec_nx = vmulq_f32(vsubq_f32(left, right), inv_2dx);
                        let vec_ny = vmulq_f32(vsubq_f32(upper, lower), inv_2dy);
                        let vec_nz = vdupq_n_f32(1.0);

                        let len_sq = vaddq_f32(
                            vaddq_f32(vmulq_f32(vec_nx, vec_nx), vmulq_f32(vec_ny, vec_ny)),
                            vmulq_f32(vec_nz, vec_nz),
                        );
                        let est = vrsqrteq_f32(len_sq);
                        let refined = vmulq_f32(vrsqrtsq_f32(vmulq_f32(len_sq, est), est), est);

                        let out = global_r * cols + global_c;
                        vst1q_f32(nx_ptr.get().add(out), vmulq_f32(vec_nx, refined));
                        vst1q_f32(ny_ptr.get().add(out), vmulq_f32(vec_ny, refined));
                        vst1q_f32(nz_ptr.get().add(out), vmulq_f32(vec_nz, refined));

                        local_c += 4;
                    }

                    // scalar tail
                    while local_c < tile_size - 1 {
                        let global_c = tc * tile_size + local_c;
                        if global_c == 0 || global_c >= cols - 1 {
                            local_c += 1;
                            continue;
                        }

                        let upper = *tile_ptr.add((local_r - 1) * tile_size + local_c) as f32;
                        let lower = *tile_ptr.add((local_r + 1) * tile_size + local_c) as f32;
                        let left = *tile_ptr.add(local_r * tile_size + (local_c - 1)) as f32;
                        let right = *tile_ptr.add(local_r * tile_size + (local_c + 1)) as f32;

                        let single_nx = (left - right) * inv_2dx_f32;
                        let single_ny = (upper - lower) * inv_2dy_f32;
                        let single_nz = 1.0f32;

                        let length = f32::sqrt(
                            single_nx * single_nx + single_ny * single_ny + single_nz * single_nz,
                        );

                        let out = global_r * cols + global_c;
                        nx_ptr.get().add(out).write(single_nx / length);
                        ny_ptr.get().add(out).write(single_ny / length);
                        nz_ptr.get().add(out).write(single_nz / length);

                        local_c += 1;
                    }
                }
            }
        });

    NormalMap {
        nx,
        ny,
        nz,
        rows,
        cols,
    }
}

// Scalar fallback for platforms without SIMD: uses get() to access tiled data.
// Slow but correct — only reached when neither NEON nor AVX2 is available.
pub fn compute_normals_scalar_tiled(hm: &dem_io::TiledHeightmap) -> NormalMap {
    let rows = hm.rows;
    let cols = hm.cols;
    let inv_2dx = 1.0f32 / (2.0 * hm.dx_meters as f32);
    let inv_2dy = 1.0f32 / (2.0 * hm.dy_meters as f32);

    let mut nx = vec![0.0f32; rows * cols];
    let mut ny = vec![0.0f32; rows * cols];
    let mut nz = vec![0.0f32; rows * cols];

    for r in 1..rows - 1 {
        for c in 1..cols - 1 {
            let east  = hm.get(r, c + 1) as f32;
            let west  = hm.get(r, c - 1) as f32;
            let south = hm.get(r + 1, c) as f32;
            let north = hm.get(r - 1, c) as f32;
            let dzdx = (east - west) * inv_2dx;
            let dzdy = (south - north) * inv_2dy;
            let len = (dzdx * dzdx + dzdy * dzdy + 1.0f32).sqrt();
            let idx = r * cols + c;
            nx[idx] = -dzdx / len;
            ny[idx] = -dzdy / len;
            nz[idx] = 1.0 / len;
        }
    }

    NormalMap { nx, ny, nz, rows, cols }
}
