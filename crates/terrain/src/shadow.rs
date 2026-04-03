use core::f32;
use std::arch::aarch64::{
    float32x4_t, uint32x4_t, vaddq_f32, vbslq_f32, vcltq_f32, vdupq_n_f32, vgetq_lane_f32,
    vld1q_f32, vmaxq_f32, vst1q_f32,
};

use dem_io::Heightmap;
use rayon::{
    iter::{IndexedParallelIterator, IntoParallelIterator, ParallelIterator},
    slice::ParallelSliceMut,
    vec,
};

pub struct ShadowMask {
    pub data: Vec<f32>,
    pub rows: usize,
    pub cols: usize,
}

pub fn compute_shadow_scalar(hm: &Heightmap, sun_elevation_rad: f32) -> ShadowMask {
    let mut data: Vec<f32> = vec![1.0f32; hm.rows * hm.cols];
    let dx: f32 = hm.dx_meters as f32;
    let tan_sun: f32 = sun_elevation_rad.tan();

    for r in 0..hm.rows {
        let mut running_max: f32 = f32::NEG_INFINITY;
        for c in 0..hm.cols {
            let height: f32 = hm.data[r * hm.cols + c] as f32;
            let dist: f32 = c as f32 * dx;
            let h_eff: f32 = height + dist * tan_sun;

            if h_eff < running_max {
                data[r * hm.cols + c] = 0.0; // in shadow
            }

            running_max = running_max.max(h_eff);
        }
    }

    ShadowMask {
        data,
        rows: hm.rows,
        cols: hm.cols,
    }
}

pub fn compute_shadow_scalar_with_azimuth(
    hm: &Heightmap,
    sun_azimuth_rad: f32,
    sun_elevation_rad: f32,
) -> ShadowMask {
    let mut data: Vec<f32> = vec![1.0f32; hm.rows * hm.cols];
    let dx: f32 = hm.dx_meters as f32;
    let tan_sun: f32 = sun_elevation_rad.tan();

    // DDA direction
    let dc: f32 = -sun_azimuth_rad.sin();
    let dr: f32 = sun_azimuth_rad.cos();

    let (dc_step, dr_step): (f32, f32) = if dc.abs() >= dr.abs() {
        (dc.signum(), dr / dc.abs())
    } else {
        (dc / dr.abs(), dr.signum())
    };

    let dy: f32 = hm.dy_meters as f32;
    let dist_per_step = ((dc_step * dx).powi(2) + (dr_step * dy).powi(2)).sqrt();

    let mut starting_pixels: Vec<(f32, f32)> = vec![];
    if dc_step > 0.0 {
        for r in 0..hm.rows {
            starting_pixels.push((r as f32, 0.0));
        }
    } else if dc_step < 0.0 {
        for r in 0..hm.rows {
            starting_pixels.push((r as f32, (hm.cols - 1) as f32));
        }
    }

    if dr_step > 0.0 {
        for c in 0..hm.cols {
            starting_pixels.push((0.0, c as f32));
        }
    } else if dr_step < 0.0 {
        for c in 0..hm.cols {
            starting_pixels.push(((hm.rows - 1) as f32, c as f32));
        }
    }

    for (start_r, start_c) in starting_pixels {
        let mut running_max: f32 = f32::NEG_INFINITY;
        let mut dist: f32 = 0.0;
        let (mut r_f, mut c_f) = (start_r as f32, start_c as f32);
        while (r_f >= 0.0 && r_f < hm.rows as f32) && (c_f >= 0.0 && c_f < hm.cols as f32) {
            let r: usize = r_f.round() as usize;
            let c: usize = c_f.round() as usize;

            let height: f32 = hm.data[r * hm.cols + c] as f32;
            let h_eff: f32 = height + dist * tan_sun;

            if h_eff < running_max {
                data[r * hm.cols + c] = 0.0; // in shadow
            }

            running_max = running_max.max(h_eff);

            r_f += dr_step;
            c_f += dc_step;
            dist += dist_per_step;
        }
    }

    ShadowMask {
        data,
        rows: hm.rows,
        cols: hm.cols,
    }
}

pub fn compute_shadow_scalar_branchless(hm: &Heightmap, sun_elevation_rad: f32) -> ShadowMask {
    let mut data: Vec<f32> = vec![1.0f32; hm.rows * hm.cols];
    let dx: f32 = hm.dx_meters as f32;
    let tan_sun: f32 = sun_elevation_rad.tan();

    for r in 0..hm.rows {
        let mut running_max: f32 = f32::NEG_INFINITY;
        for c in 0..hm.cols {
            let height: f32 = hm.data[r * hm.cols + c] as f32;
            let dist: f32 = c as f32 * dx;
            let h_eff: f32 = height + dist * tan_sun;

            let in_shadow = (h_eff < running_max) as u32 as f32; // 0.0 or 1.0

            data[r * hm.cols + c] = 1.0 - in_shadow;

            running_max = running_max.max(h_eff);
        }
    }

    ShadowMask {
        data,
        rows: hm.rows,
        cols: hm.cols,
    }
}

pub unsafe fn compute_shadow_neon(hm: &Heightmap, sun_elevation_rad: f32) -> ShadowMask {
    let mut data: Vec<f32> = vec![1.0f32; hm.rows * hm.cols];
    let dx: f32 = hm.dx_meters as f32;
    let tan_sun: f32 = sun_elevation_rad.tan();

    let mut r: usize = 0usize;
    while r + 4 < hm.rows {
        let base: [usize; 4] = [
            (r + 0) * hm.cols,
            (r + 1) * hm.cols,
            (r + 2) * hm.cols,
            (r + 3) * hm.cols,
        ];
        let mut running_max: float32x4_t = vdupq_n_f32(f32::NEG_INFINITY);
        let step: f32 = dx * tan_sun;
        for c in 0..hm.cols {
            let heights: [f32; 4] = [
                hm.data[base[0] + c] as f32,
                hm.data[base[1] + c] as f32,
                hm.data[base[2] + c] as f32,
                hm.data[base[3] + c] as f32,
            ];
            unsafe {
                let h_vec: float32x4_t = vld1q_f32(heights.as_ptr());

                let h_eff: float32x4_t = vaddq_f32(h_vec, vdupq_n_f32(c as f32 * step));
                let shadow_mask: uint32x4_t = vcltq_f32(h_eff, running_max);
                let result: float32x4_t =
                    vbslq_f32(shadow_mask, vdupq_n_f32(0.0), vdupq_n_f32(1.0));

                *data.get_unchecked_mut(base[0] + c) = vgetq_lane_f32::<0>(result);
                *data.get_unchecked_mut(base[1] + c) = vgetq_lane_f32::<1>(result);
                *data.get_unchecked_mut(base[2] + c) = vgetq_lane_f32::<2>(result);
                *data.get_unchecked_mut(base[3] + c) = vgetq_lane_f32::<3>(result);

                running_max = vmaxq_f32(running_max, h_eff);
            }
        }

        r += 4;
    }

    while r < hm.rows {
        let mut running_max: f32 = f32::NEG_INFINITY;
        for c in 0..hm.cols {
            let height: f32 = hm.data[r * hm.cols + c] as f32;
            let dist: f32 = c as f32 * dx;
            let h_eff: f32 = height + dist * tan_sun;

            if h_eff < running_max {
                data[r * hm.cols + c] = 0.0; // in shadow
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

pub unsafe fn compute_shadow_neon_parallel(hm: &Heightmap, sun_elevation_rad: f32) -> ShadowMask {
    let mut data: Vec<f32> = vec![1.0f32; hm.rows * hm.cols];
    let dx: f32 = hm.dx_meters as f32;
    let tan_sun: f32 = sun_elevation_rad.tan();

    data.par_chunks_mut(4 * hm.cols)
        .enumerate()
        .for_each(|(chunk_idx, chunk)| {
            if chunk.len() / hm.cols != 4 {
                let rows_in_chunk: usize = chunk.len() / hm.cols;
                for local_r in 0..rows_in_chunk {
                    let global_r: usize = chunk_idx * 4 + local_r;
                    let mut running_max: f32 = f32::NEG_INFINITY;
                    for c in 0..hm.cols {
                        let height: f32 = hm.data[global_r * hm.cols + c] as f32;
                        let dist: f32 = c as f32 * dx;
                        let h_eff: f32 = height + dist * tan_sun;

                        if h_eff < running_max {
                            chunk[local_r * hm.cols + c] = 0.0; // in shadow
                        }

                        running_max = running_max.max(h_eff);
                    }
                }

                return;
            }

            let r: usize = chunk_idx * 4;

            let base: [usize; 4] = [
                (r + 0) * hm.cols,
                (r + 1) * hm.cols,
                (r + 2) * hm.cols,
                (r + 3) * hm.cols,
            ];
            let mut running_max: float32x4_t = vdupq_n_f32(f32::NEG_INFINITY);
            let step: f32 = dx * tan_sun;
            for c in 0..hm.cols {
                let heights: [f32; 4] = [
                    hm.data[base[0] + c] as f32,
                    hm.data[base[1] + c] as f32,
                    hm.data[base[2] + c] as f32,
                    hm.data[base[3] + c] as f32,
                ];
                unsafe {
                    let h_vec: float32x4_t = vld1q_f32(heights.as_ptr());

                    let h_eff: float32x4_t = vaddq_f32(h_vec, vdupq_n_f32(c as f32 * step));
                    let shadow_mask: uint32x4_t = vcltq_f32(h_eff, running_max);
                    let result: float32x4_t =
                        vbslq_f32(shadow_mask, vdupq_n_f32(0.0), vdupq_n_f32(1.0));

                    *chunk.get_unchecked_mut(0 * hm.cols + c) = vgetq_lane_f32::<0>(result);
                    *chunk.get_unchecked_mut(1 * hm.cols + c) = vgetq_lane_f32::<1>(result);
                    *chunk.get_unchecked_mut(2 * hm.cols + c) = vgetq_lane_f32::<2>(result);
                    *chunk.get_unchecked_mut(3 * hm.cols + c) = vgetq_lane_f32::<3>(result);

                    running_max = vmaxq_f32(running_max, h_eff);
                }
            }
        });

    ShadowMask {
        data,
        rows: hm.rows,
        cols: hm.cols,
    }
}
