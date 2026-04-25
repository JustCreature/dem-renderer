#![cfg(target_arch = "x86_64")]

use rayon::prelude::*;

use crate::{NormalMap, SendPtr};

// AVX2 rsqrt + one Newton-Raphson refinement step.
// NEON uses vrsqrteq_f32 (11-bit) + vrsqrtsq_f32 NR step.
// AVX2 uses _mm256_rsqrt_ps (~14-bit) + one NR step: y1 = y0 * (1.5 - 0.5 * x * y0^2)
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

// Load 8 consecutive i16 values from ptr and convert to f32.
// NEON: vcvtq_f32_s32(vmovl_s16(vld1_s16(ptr)))  [4-wide]
// AVX2: _mm256_cvtepi32_ps(_mm256_cvtepi16_epi32(_mm_loadu_si128(ptr)))  [8-wide]
#[inline(always)]
unsafe fn load_i16_to_f32_avx2(ptr: *const i16) -> core::arch::x86_64::__m256 {
    use core::arch::x86_64::*;
    let raw = _mm_loadu_si128(ptr as *const __m128i);
    _mm256_cvtepi32_ps(_mm256_cvtepi16_epi32(raw))
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub unsafe fn compute_normals_avx2(hm: &dem_io::Heightmap) -> NormalMap {
    use core::arch::x86_64::*;

    let mut nx: Vec<f32> = vec![0.0f32; hm.rows * hm.cols];
    let mut ny: Vec<f32> = vec![0.0f32; hm.rows * hm.cols];
    let mut nz: Vec<f32> = vec![0.0f32; hm.rows * hm.cols];

    let inv_2dx = _mm256_set1_ps(1.0 / (2.0 * hm.dx_meters as f32));
    let inv_2dy = _mm256_set1_ps(1.0 / (2.0 * hm.dy_meters as f32));
    let one = _mm256_set1_ps(1.0);

    for r in 1..hm.rows - 1 {
        let mut c = 1usize;
        while c + 8 < hm.cols - 1 {
            let upper = load_i16_to_f32_avx2(hm.data.as_ptr().add((r - 1) * hm.cols + c));
            let lower = load_i16_to_f32_avx2(hm.data.as_ptr().add((r + 1) * hm.cols + c));
            let left = load_i16_to_f32_avx2(hm.data.as_ptr().add(r * hm.cols + (c - 1)));
            let right = load_i16_to_f32_avx2(hm.data.as_ptr().add(r * hm.cols + (c + 1)));

            let vec_nx = _mm256_mul_ps(_mm256_sub_ps(left, right), inv_2dx);
            let vec_ny = _mm256_mul_ps(_mm256_sub_ps(upper, lower), inv_2dy);
            let vec_nz = one;

            let len_sq = _mm256_add_ps(
                _mm256_add_ps(_mm256_mul_ps(vec_nx, vec_nx), _mm256_mul_ps(vec_ny, vec_ny)),
                _mm256_mul_ps(vec_nz, vec_nz),
            );
            let inv_len = rsqrt_nr(len_sq);

            _mm256_storeu_ps(
                nx.as_mut_ptr().add(r * hm.cols + c),
                _mm256_mul_ps(vec_nx, inv_len),
            );
            _mm256_storeu_ps(
                ny.as_mut_ptr().add(r * hm.cols + c),
                _mm256_mul_ps(vec_ny, inv_len),
            );
            _mm256_storeu_ps(
                nz.as_mut_ptr().add(r * hm.cols + c),
                _mm256_mul_ps(vec_nz, inv_len),
            );

            c += 8;
        }
        // scalar tail
        while c < hm.cols - 1 {
            let upper = hm.data[(r - 1) * hm.cols + c] as f32;
            let lower = hm.data[(r + 1) * hm.cols + c] as f32;
            let left = hm.data[r * hm.cols + (c - 1)] as f32;
            let right = hm.data[r * hm.cols + (c + 1)] as f32;

            let single_nx = (left - right) / (2.0 * hm.dx_meters as f32);
            let single_ny = (upper - lower) / (2.0 * hm.dy_meters as f32);
            let single_nz = 1.0f32;
            let length =
                f32::sqrt(single_nx * single_nx + single_ny * single_ny + single_nz * single_nz);

            nx[r * hm.cols + c] = single_nx / length;
            ny[r * hm.cols + c] = single_ny / length;
            nz[r * hm.cols + c] = single_nz / length;
            c += 1;
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
pub unsafe fn compute_normals_avx2_parallel(hm: &dem_io::Heightmap) -> NormalMap {
    use core::arch::x86_64::*;

    let mut nx: Vec<f32> = vec![0.0f32; hm.rows * hm.cols];
    let mut ny: Vec<f32> = vec![0.0f32; hm.rows * hm.cols];
    let mut nz: Vec<f32> = vec![0.0f32; hm.rows * hm.cols];

    let nx_ptr = SendPtr(nx.as_mut_ptr());
    let ny_ptr = SendPtr(ny.as_mut_ptr());
    let nz_ptr = SendPtr(nz.as_mut_ptr());

    let inv_2dx_f32 = 1.0f32 / (2.0 * hm.dx_meters as f32);
    let inv_2dy_f32 = 1.0f32 / (2.0 * hm.dy_meters as f32);

    (1..hm.rows - 1).into_par_iter().for_each(|r| {
        let inv_2dx = _mm256_set1_ps(inv_2dx_f32);
        let inv_2dy = _mm256_set1_ps(inv_2dy_f32);
        let one = _mm256_set1_ps(1.0);

        let nx_p = nx_ptr.get();
        let ny_p = ny_ptr.get();
        let nz_p = nz_ptr.get();

        let mut c = 1usize;
        while c + 8 < hm.cols - 1 {
            let upper = load_i16_to_f32_avx2(hm.data.as_ptr().add((r - 1) * hm.cols + c));
            let lower = load_i16_to_f32_avx2(hm.data.as_ptr().add((r + 1) * hm.cols + c));
            let left = load_i16_to_f32_avx2(hm.data.as_ptr().add(r * hm.cols + (c - 1)));
            let right = load_i16_to_f32_avx2(hm.data.as_ptr().add(r * hm.cols + (c + 1)));

            let vec_nx = _mm256_mul_ps(_mm256_sub_ps(left, right), inv_2dx);
            let vec_ny = _mm256_mul_ps(_mm256_sub_ps(upper, lower), inv_2dy);
            let vec_nz = one;

            let len_sq = _mm256_add_ps(
                _mm256_add_ps(_mm256_mul_ps(vec_nx, vec_nx), _mm256_mul_ps(vec_ny, vec_ny)),
                _mm256_mul_ps(vec_nz, vec_nz),
            );
            let inv_len = rsqrt_nr(len_sq);

            _mm256_storeu_ps(nx_p.add(r * hm.cols + c), _mm256_mul_ps(vec_nx, inv_len));
            _mm256_storeu_ps(ny_p.add(r * hm.cols + c), _mm256_mul_ps(vec_ny, inv_len));
            _mm256_storeu_ps(nz_p.add(r * hm.cols + c), _mm256_mul_ps(vec_nz, inv_len));

            c += 8;
        }
        // scalar tail
        while c < hm.cols - 1 {
            let upper = hm.data[(r - 1) * hm.cols + c] as f32;
            let lower = hm.data[(r + 1) * hm.cols + c] as f32;
            let left = hm.data[r * hm.cols + (c - 1)] as f32;
            let right = hm.data[r * hm.cols + (c + 1)] as f32;

            let single_nx = (left - right) * inv_2dx_f32;
            let single_ny = (upper - lower) * inv_2dy_f32;
            let single_nz = 1.0f32;
            let length =
                f32::sqrt(single_nx * single_nx + single_ny * single_ny + single_nz * single_nz);

            nx_p.add(r * hm.cols + c).write(single_nx / length);
            ny_p.add(r * hm.cols + c).write(single_ny / length);
            nz_p.add(r * hm.cols + c).write(single_nz / length);
            c += 1;
        }
    });

    NormalMap {
        nx,
        ny,
        nz,
        rows: hm.rows,
        cols: hm.cols,
    }
}
