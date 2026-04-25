#![cfg(target_arch = "x86_64")]

use crate::SendPtr;
use crate::shadow::{DdaSetup, ShadowMask, dda_setup};
use dem_io::Heightmap;
use rayon::slice::ParallelSliceMut;
use rayon::{
    iter::{IndexedParallelIterator, ParallelIterator},
};

// ── AVX2 8-wide west-only ─────────────────────────────────────────────────────
//
// Processes 8 rows at a time in SIMD lanes (same idea as NEON 4-wide, but wider).
// Running-max and shadow state are tracked across 8 rows simultaneously.

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub unsafe fn compute_shadow_avx2(hm: &Heightmap, sun_elevation_rad: f32) -> ShadowMask {
    use core::arch::x86_64::*;

    let mut data: Vec<f32> = vec![1.0f32; hm.rows * hm.cols];
    let dx = hm.dx_meters as f32;
    let tan_sun = sun_elevation_rad.tan();
    let step = dx * tan_sun;

    let mut r = 0usize;
    while r + 8 <= hm.rows {
        let base = [
            r * hm.cols,
            (r + 1) * hm.cols,
            (r + 2) * hm.cols,
            (r + 3) * hm.cols,
            (r + 4) * hm.cols,
            (r + 5) * hm.cols,
            (r + 6) * hm.cols,
            (r + 7) * hm.cols,
        ];
        let mut running_max = _mm256_set1_ps(f32::NEG_INFINITY);

        for c in 0..hm.cols {
            let heights = [
                hm.data[base[0] + c] as f32,
                hm.data[base[1] + c] as f32,
                hm.data[base[2] + c] as f32,
                hm.data[base[3] + c] as f32,
                hm.data[base[4] + c] as f32,
                hm.data[base[5] + c] as f32,
                hm.data[base[6] + c] as f32,
                hm.data[base[7] + c] as f32,
            ];
            let h_vec = _mm256_loadu_ps(heights.as_ptr());
            let h_eff = _mm256_add_ps(h_vec, _mm256_set1_ps(c as f32 * step));
            // mask is set (all 1-bits) for lanes where h_eff < running_max (in shadow)
            let mask = _mm256_cmp_ps::<1>(h_eff, running_max); // _CMP_LT_OS = 1
            // blendv: select 0.0 (in shadow) where mask bit is set, 1.0 (lit) otherwise
            let result = _mm256_blendv_ps(_mm256_set1_ps(1.0), _mm256_set1_ps(0.0), mask);

            let mut result_arr = [0.0f32; 8];
            _mm256_storeu_ps(result_arr.as_mut_ptr(), result);
            for i in 0..8 {
                *data.get_unchecked_mut(base[i] + c) = result_arr[i];
            }

            running_max = _mm256_max_ps(running_max, h_eff);
        }
        r += 8;
    }

    // scalar remainder rows
    while r < hm.rows {
        let mut running_max = f32::NEG_INFINITY;
        for c in 0..hm.cols {
            let h_eff = hm.data[r * hm.cols + c] as f32 + c as f32 * step;
            if h_eff < running_max {
                data[r * hm.cols + c] = 0.0;
            }
            running_max = running_max.max(h_eff);
        }
        r += 1;
    }

    ShadowMask {
        data,
        rows: hm.rows,
        cols: hm.cols,
    }
}

// ── AVX2 parallel 8-wide west-only ───────────────────────────────────────────

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub unsafe fn compute_shadow_avx2_parallel(hm: &Heightmap, sun_elevation_rad: f32) -> ShadowMask {
    use core::arch::x86_64::*;

    let mut data: Vec<f32> = vec![1.0f32; hm.rows * hm.cols];
    let dx = hm.dx_meters as f32;
    let tan_sun = sun_elevation_rad.tan();
    let step = dx * tan_sun;

    data.par_chunks_mut(8 * hm.cols)
        .enumerate()
        .for_each(|(chunk_idx, chunk)| {
            let rows_in_chunk = chunk.len() / hm.cols;

            if rows_in_chunk < 8 {
                // scalar remainder chunk
                for local_r in 0..rows_in_chunk {
                    let global_r = chunk_idx * 8 + local_r;
                    let mut running_max = f32::NEG_INFINITY;
                    for c in 0..hm.cols {
                        let h_eff = hm.data[global_r * hm.cols + c] as f32 + c as f32 * step;
                        if h_eff < running_max {
                            chunk[local_r * hm.cols + c] = 0.0;
                        }
                        running_max = running_max.max(h_eff);
                    }
                }
                return;
            }

            let r = chunk_idx * 8;
            let base = [
                r * hm.cols,
                (r + 1) * hm.cols,
                (r + 2) * hm.cols,
                (r + 3) * hm.cols,
                (r + 4) * hm.cols,
                (r + 5) * hm.cols,
                (r + 6) * hm.cols,
                (r + 7) * hm.cols,
            ];
            let mut running_max = _mm256_set1_ps(f32::NEG_INFINITY);

            for c in 0..hm.cols {
                let heights = [
                    hm.data[base[0] + c] as f32,
                    hm.data[base[1] + c] as f32,
                    hm.data[base[2] + c] as f32,
                    hm.data[base[3] + c] as f32,
                    hm.data[base[4] + c] as f32,
                    hm.data[base[5] + c] as f32,
                    hm.data[base[6] + c] as f32,
                    hm.data[base[7] + c] as f32,
                ];
                let h_vec = _mm256_loadu_ps(heights.as_ptr());
                let h_eff = _mm256_add_ps(h_vec, _mm256_set1_ps(c as f32 * step));
                let mask = _mm256_cmp_ps::<1>(h_eff, running_max);
                let result = _mm256_blendv_ps(_mm256_set1_ps(1.0), _mm256_set1_ps(0.0), mask);

                let mut result_arr = [0.0f32; 8];
                _mm256_storeu_ps(result_arr.as_mut_ptr(), result);
                for i in 0..8 {
                    *chunk.get_unchecked_mut(i * hm.cols + c) = result_arr[i];
                }

                running_max = _mm256_max_ps(running_max, h_eff);
            }
        });

    ShadowMask {
        data,
        rows: hm.rows,
        cols: hm.cols,
    }
}

// ── AVX2 parallel arbitrary azimuth (DDA) ────────────────────────────────────
//
// Extends the NEON 4-ray DDA approach to 8 rays per SIMD group.
// Parallelises over groups of 8 starting pixels via rayon.
// Within each group, 8 rays run simultaneously in AVX2 lanes.

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub unsafe fn compute_shadow_avx2_parallel_with_azimuth(
    hm: &Heightmap,
    sun_azimuth_rad: f32,
    sun_elevation_rad: f32,
) -> ShadowMask {
    use core::arch::x86_64::*;

    let mut data: Vec<f32> = vec![1.0f32; hm.rows * hm.cols];
    let dx = hm.dx_meters as f32;
    let dy = hm.dy_meters as f32;
    let tan_sun = sun_elevation_rad.tan();

    let DdaSetup {
        dc_step,
        dr_step,
        dist_per_step,
        mut starting_pixels,
    } = dda_setup(hm.rows, hm.cols, sun_azimuth_rad, dx, dy);

    let data_ptr = SendPtr(data.as_mut_ptr());

    starting_pixels.par_chunks_mut(8).for_each(|rays| {
        let ptr = data_ptr.get();

        // ── scalar fallback for partial chunks (< 8 rays) ────────────────
        if rays.len() < 8 {
            for (sr, sc) in rays.iter().copied() {
                let mut rm = f32::NEG_INFINITY;
                let mut dist = 0.0f32;
                let (mut rf, mut cf) = (sr, sc);
                while rf >= 0.0 && rf < hm.rows as f32 && cf >= 0.0 && cf < hm.cols as f32 {
                    let r = rf.round() as usize;
                    let c = cf.round() as usize;
                    let h_eff = hm.data[r * hm.cols + c] as f32 + dist * tan_sun;
                    if h_eff < rm {
                        *ptr.add(r * hm.cols + c) = 0.0;
                    }
                    rm = rm.max(h_eff);
                    rf += dr_step;
                    cf += dc_step;
                    dist += dist_per_step;
                }
            }
            return;
        }

        // ── AVX2: 8 rays in parallel ──────────────────────────────────────
        let mut rf = [
            rays[0].0, rays[1].0, rays[2].0, rays[3].0,
            rays[4].0, rays[5].0, rays[6].0, rays[7].0,
        ];
        let mut cf = [
            rays[0].1, rays[1].1, rays[2].1, rays[3].1,
            rays[4].1, rays[5].1, rays[6].1, rays[7].1,
        ];
        let mut dist = 0.0f32;
        let mut running_max = _mm256_set1_ps(f32::NEG_INFINITY);

        // Run AVX2 while ALL 8 rays are still inside the grid.
        while rf
            .iter()
            .zip(cf.iter())
            .all(|(&r, &c)| r >= 0.0 && r < hm.rows as f32 && c >= 0.0 && c < hm.cols as f32)
        {
            let heights = [
                hm.data[rf[0].round() as usize * hm.cols + cf[0].round() as usize] as f32,
                hm.data[rf[1].round() as usize * hm.cols + cf[1].round() as usize] as f32,
                hm.data[rf[2].round() as usize * hm.cols + cf[2].round() as usize] as f32,
                hm.data[rf[3].round() as usize * hm.cols + cf[3].round() as usize] as f32,
                hm.data[rf[4].round() as usize * hm.cols + cf[4].round() as usize] as f32,
                hm.data[rf[5].round() as usize * hm.cols + cf[5].round() as usize] as f32,
                hm.data[rf[6].round() as usize * hm.cols + cf[6].round() as usize] as f32,
                hm.data[rf[7].round() as usize * hm.cols + cf[7].round() as usize] as f32,
            ];
            let h_vec = _mm256_loadu_ps(heights.as_ptr());
            let h_eff = _mm256_add_ps(h_vec, _mm256_set1_ps(dist * tan_sun));
            let mask = _mm256_cmp_ps::<1>(h_eff, running_max);
            let result = _mm256_blendv_ps(_mm256_set1_ps(1.0), _mm256_set1_ps(0.0), mask);

            let mut result_arr = [0.0f32; 8];
            _mm256_storeu_ps(result_arr.as_mut_ptr(), result);
            for i in 0..8 {
                let idx = rf[i].round() as usize * hm.cols + cf[i].round() as usize;
                *ptr.add(idx) = result_arr[i];
            }

            running_max = _mm256_max_ps(running_max, h_eff);

            for i in 0..8 {
                rf[i] += dr_step;
                cf[i] += dc_step;
            }
            dist += dist_per_step;
        }

        // Extract per-lane running_max and finish any rays still in bounds.
        let mut per_max = [0.0f32; 8];
        _mm256_storeu_ps(per_max.as_mut_ptr(), running_max);

        for i in 0..8 {
            let mut rm = per_max[i];
            let mut r_f = rf[i];
            let mut c_f = cf[i];
            let mut d = dist;
            while r_f >= 0.0 && r_f < hm.rows as f32 && c_f >= 0.0 && c_f < hm.cols as f32 {
                let r = r_f.round() as usize;
                let c = c_f.round() as usize;
                let h_eff = hm.data[r * hm.cols + c] as f32 + d * tan_sun;
                if h_eff < rm {
                    *ptr.add(r * hm.cols + c) = 0.0;
                }
                rm = rm.max(h_eff);
                r_f += dr_step;
                c_f += dc_step;
                d += dist_per_step;
            }
        }
    });

    ShadowMask {
        data,
        rows: hm.rows,
        cols: hm.cols,
    }
}
