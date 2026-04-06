#![cfg(target_arch = "x86_64")]

use rayon::prelude::*;

use crate::{NormalMap, SendPtr};

#[inline(always)]
unsafe fn load_i16_to_f32_avx2(ptr: *const i16) -> core::arch::x86_64::__m256 {
    use core::arch::x86_64::*;
    let raw = _mm_loadu_si128(ptr as *const __m128i);
    _mm256_cvtepi32_ps(_mm256_cvtepi16_epi32(raw))
}

#[inline(always)]
unsafe fn rsqrt_nr(len_sq: core::arch::x86_64::__m256) -> core::arch::x86_64::__m256 {
    use core::arch::x86_64::*;
    let est = _mm256_rsqrt_ps(len_sq);
    let half_x_est_sq = _mm256_mul_ps(
        _mm256_set1_ps(0.5),
        _mm256_mul_ps(len_sq, _mm256_mul_ps(est, est)),
    );
    _mm256_mul_ps(est, _mm256_sub_ps(_mm256_set1_ps(1.5), half_x_est_sq))
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub unsafe fn compute_normals_avx2_tiled(hm: &dem_io::TiledHeightmap) -> NormalMap {
    use core::arch::x86_64::*;

    let mut nx: Vec<f32> = vec![0.0f32; hm.rows * hm.cols];
    let mut ny: Vec<f32> = vec![0.0f32; hm.rows * hm.cols];
    let mut nz: Vec<f32> = vec![0.0f32; hm.rows * hm.cols];

    let inv_2dx_f32 = 1.0f32 / (2.0 * hm.dx_meters as f32);
    let inv_2dy_f32 = 1.0f32 / (2.0 * hm.dy_meters as f32);

    let inv_2dx = _mm256_set1_ps(inv_2dx_f32);
    let inv_2dy = _mm256_set1_ps(inv_2dy_f32);
    let one = _mm256_set1_ps(1.0);

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

                while local_c + 8 < hm.tile_size - 1 {
                    let global_c = tc * hm.tile_size + local_c;
                    if global_c + 8 >= hm.cols {
                        break;
                    }

                    let upper =
                        load_i16_to_f32_avx2(tile_ptr.add((local_r - 1) * hm.tile_size + local_c));
                    let lower =
                        load_i16_to_f32_avx2(tile_ptr.add((local_r + 1) * hm.tile_size + local_c));
                    let left =
                        load_i16_to_f32_avx2(tile_ptr.add(local_r * hm.tile_size + (local_c - 1)));
                    let right =
                        load_i16_to_f32_avx2(tile_ptr.add(local_r * hm.tile_size + (local_c + 1)));

                    let vec_nx = _mm256_mul_ps(_mm256_sub_ps(left, right), inv_2dx);
                    let vec_ny = _mm256_mul_ps(_mm256_sub_ps(upper, lower), inv_2dy);
                    let vec_nz = one;

                    let len_sq = _mm256_add_ps(
                        _mm256_add_ps(
                            _mm256_mul_ps(vec_nx, vec_nx),
                            _mm256_mul_ps(vec_ny, vec_ny),
                        ),
                        _mm256_mul_ps(vec_nz, vec_nz),
                    );
                    let inv_len = rsqrt_nr(len_sq);

                    let out = global_r * hm.cols + global_c;
                    _mm256_storeu_ps(nx.as_mut_ptr().add(out), _mm256_mul_ps(vec_nx, inv_len));
                    _mm256_storeu_ps(ny.as_mut_ptr().add(out), _mm256_mul_ps(vec_ny, inv_len));
                    _mm256_storeu_ps(nz.as_mut_ptr().add(out), _mm256_mul_ps(vec_nz, inv_len));

                    local_c += 8;
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

    NormalMap {
        nx,
        ny,
        nz,
        rows: hm.rows,
        cols: hm.cols,
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub unsafe fn compute_normals_avx2_tiled_parallel(hm: &dem_io::TiledHeightmap) -> NormalMap {
    use core::arch::x86_64::*;

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
    (0..tile_rows * tile_cols)
        .into_par_iter()
        .for_each(move |tile_idx| {
            let tr = tile_idx / tile_cols;
            let tc = tile_idx % tile_cols;

            let inv_2dx = _mm256_set1_ps(inv_2dx_f32);
            let inv_2dy = _mm256_set1_ps(inv_2dy_f32);
            let one = _mm256_set1_ps(1.0);

            let tile_start = (tr * tile_cols + tc) * tile_size * tile_size;
            let tile_ptr = hm.tiles().as_ptr().add(tile_start);

            for local_r in 1..tile_size - 1 {
                let global_r = tr * tile_size + local_r;
                if global_r >= rows - 1 {
                    continue;
                }

                let mut local_c = 1usize;

                while local_c + 8 < tile_size - 1 {
                    let global_c = tc * tile_size + local_c;
                    if global_c + 8 >= cols {
                        break;
                    }

                    let upper = load_i16_to_f32_avx2(
                        tile_ptr.add((local_r - 1) * tile_size + local_c),
                    );
                    let lower = load_i16_to_f32_avx2(
                        tile_ptr.add((local_r + 1) * tile_size + local_c),
                    );
                    let left = load_i16_to_f32_avx2(
                        tile_ptr.add(local_r * tile_size + (local_c - 1)),
                    );
                    let right = load_i16_to_f32_avx2(
                        tile_ptr.add(local_r * tile_size + (local_c + 1)),
                    );

                    let vec_nx = _mm256_mul_ps(_mm256_sub_ps(left, right), inv_2dx);
                    let vec_ny = _mm256_mul_ps(_mm256_sub_ps(upper, lower), inv_2dy);
                    let vec_nz = one;

                    let len_sq = _mm256_add_ps(
                        _mm256_add_ps(
                            _mm256_mul_ps(vec_nx, vec_nx),
                            _mm256_mul_ps(vec_ny, vec_ny),
                        ),
                        _mm256_mul_ps(vec_nz, vec_nz),
                    );
                    let inv_len = rsqrt_nr(len_sq);

                    let out = global_r * cols + global_c;
                    _mm256_storeu_ps(
                        nx_ptr.get().add(out),
                        _mm256_mul_ps(vec_nx, inv_len),
                    );
                    _mm256_storeu_ps(
                        ny_ptr.get().add(out),
                        _mm256_mul_ps(vec_ny, inv_len),
                    );
                    _mm256_storeu_ps(
                        nz_ptr.get().add(out),
                        _mm256_mul_ps(vec_nz, inv_len),
                    );

                    local_c += 8;
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
        });

    NormalMap {
        nx,
        ny,
        nz,
        rows,
        cols,
    }
}
