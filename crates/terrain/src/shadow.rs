use core::f32;

use dem_io::Heightmap;

// ── shared DDA helpers ───────────────────────────────────────────────────────

pub(crate) struct DdaSetup {
    pub(crate) dc_step: f32,
    pub(crate) dr_step: f32,
    pub(crate) dist_per_step: f32,
    pub(crate) starting_pixels: Vec<(f32, f32)>,
}

pub(crate) fn dda_setup(
    rows: usize,
    cols: usize,
    sun_azimuth_rad: f32,
    dx: f32,
    dy: f32,
) -> DdaSetup {
    let dc = -sun_azimuth_rad.sin();
    let dr = sun_azimuth_rad.cos();

    // Normalise: dominant axis steps by exactly ±1, other axis follows fractionally.
    let (dc_step, dr_step) = if dc.abs() >= dr.abs() {
        (dc.signum(), dr / dc.abs())
    } else {
        (dc / dr.abs(), dr.signum())
    };

    // Euclidean ground distance covered per DDA step.
    let dist_per_step = ((dc_step * dx).powi(2) + (dr_step * dy).powi(2)).sqrt();

    // Starting pixels on the entry edge(s) — two edges apply for diagonal sun.
    let mut starting_pixels: Vec<(f32, f32)> = Vec::new();
    if dc_step > 0.0 {
        for r in 0..rows {
            starting_pixels.push((r as f32, 0.0));
        }
    } else if dc_step < 0.0 {
        for r in 0..rows {
            starting_pixels.push((r as f32, (cols - 1) as f32));
        }
    }
    if dr_step > 0.0 {
        for c in 0..cols {
            starting_pixels.push((0.0, c as f32));
        }
    } else if dr_step < 0.0 {
        for c in 0..cols {
            starting_pixels.push(((rows - 1) as f32, c as f32));
        }
    }

    DdaSetup {
        dc_step,
        dr_step,
        dist_per_step,
        starting_pixels,
    }
}

// ── public output type ───────────────────────────────────────────────────────

pub struct ShadowMask {
    pub data: Vec<f32>,
    pub rows: usize,
    pub cols: usize,
}

// ── scalar west-only (phase 3 baseline) ─────────────────────────────────────

pub fn compute_shadow_scalar(hm: &Heightmap, sun_elevation_rad: f32) -> ShadowMask {
    let mut data: Vec<f32> = vec![1.0f32; hm.rows * hm.cols];
    let dx: f32 = hm.dx_meters as f32;
    let tan_sun: f32 = sun_elevation_rad.tan();

    for r in 0..hm.rows {
        let mut running_max: f32 = f32::NEG_INFINITY;
        for c in 0..hm.cols {
            let height: f32 = hm.data[r * hm.cols + c];
            let dist: f32 = c as f32 * dx;
            let h_eff: f32 = height + dist * tan_sun;

            if h_eff < running_max {
                data[r * hm.cols + c] = 0.0;
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

// ── scalar branchless west-only ──────────────────────────────────────────────

pub fn compute_shadow_scalar_branchless(hm: &Heightmap, sun_elevation_rad: f32) -> ShadowMask {
    let mut data: Vec<f32> = vec![1.0f32; hm.rows * hm.cols];
    let dx: f32 = hm.dx_meters as f32;
    let tan_sun: f32 = sun_elevation_rad.tan();

    for r in 0..hm.rows {
        let mut running_max: f32 = f32::NEG_INFINITY;
        for c in 0..hm.cols {
            let height: f32 = hm.data[r * hm.cols + c];
            let dist: f32 = c as f32 * dx;
            let h_eff: f32 = height + dist * tan_sun;

            let in_shadow = (h_eff < running_max) as u32 as f32;
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

// ── scalar arbitrary azimuth (DDA) ───────────────────────────────────────────

pub fn compute_shadow_scalar_with_azimuth(
    hm: &Heightmap,
    sun_azimuth_rad: f32,
    sun_elevation_rad: f32,
    penumbra_meters: f32,
) -> ShadowMask {
    let mut data: Vec<f32> = vec![1.0f32; hm.rows * hm.cols];
    let dx: f32 = hm.dx_meters as f32;
    let dy: f32 = hm.dy_meters as f32;
    let tan_sun: f32 = sun_elevation_rad.tan();

    let DdaSetup {
        dc_step,
        dr_step,
        dist_per_step,
        starting_pixels,
    } = dda_setup(hm.rows, hm.cols, sun_azimuth_rad, dx, dy);

    for (start_r, start_c) in starting_pixels {
        let mut running_max: f32 = f32::NEG_INFINITY;
        let mut dist: f32 = 0.0;
        let (mut r_f, mut c_f) = (start_r, start_c);

        while r_f >= 0.0 && r_f < hm.rows as f32 && c_f >= 0.0 && c_f < hm.cols as f32 {
            let r = r_f.floor() as usize;
            let c = c_f.floor() as usize;
            let h_eff = hm.data[r * hm.cols + c] + dist * tan_sun;

            if h_eff < running_max {
                let margin = running_max - h_eff;
                data[r * hm.cols + c] = (1.0 - margin / penumbra_meters).max(0.0);
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

// ── NEON 4-wide west-only ────────────────────────────────────────────────────

#[cfg(target_arch = "aarch64")]
pub unsafe fn compute_shadow_neon(hm: &Heightmap, sun_elevation_rad: f32) -> ShadowMask {
    use std::arch::aarch64::{
        float32x4_t, uint32x4_t, vaddq_f32, vbslq_f32, vcltq_f32, vdupq_n_f32, vgetq_lane_f32,
        vld1q_f32, vmaxq_f32,
    };
    let mut data: Vec<f32> = vec![1.0f32; hm.rows * hm.cols];
    let dx: f32 = hm.dx_meters as f32;
    let tan_sun: f32 = sun_elevation_rad.tan();
    let step: f32 = dx * tan_sun;

    let mut r: usize = 0;
    while r + 4 <= hm.rows {
        let base = [
            r * hm.cols,
            (r + 1) * hm.cols,
            (r + 2) * hm.cols,
            (r + 3) * hm.cols,
        ];
        let mut running_max: float32x4_t = unsafe { vdupq_n_f32(f32::NEG_INFINITY) };

        for c in 0..hm.cols {
            let heights = [
                hm.data[base[0] + c],
                hm.data[base[1] + c],
                hm.data[base[2] + c],
                hm.data[base[3] + c],
            ];
            unsafe {
                let h_vec = vld1q_f32(heights.as_ptr());
                let h_eff = vaddq_f32(h_vec, vdupq_n_f32(c as f32 * step));
                let mask: uint32x4_t = vcltq_f32(h_eff, running_max);
                let result = vbslq_f32(mask, vdupq_n_f32(0.0), vdupq_n_f32(1.0));

                *data.get_unchecked_mut(base[0] + c) = vgetq_lane_f32::<0>(result);
                *data.get_unchecked_mut(base[1] + c) = vgetq_lane_f32::<1>(result);
                *data.get_unchecked_mut(base[2] + c) = vgetq_lane_f32::<2>(result);
                *data.get_unchecked_mut(base[3] + c) = vgetq_lane_f32::<3>(result);

                running_max = vmaxq_f32(running_max, h_eff);
            }
        }
        r += 4;
    }

    // scalar remainder
    while r < hm.rows {
        let mut running_max = f32::NEG_INFINITY;
        for c in 0..hm.cols {
            let h_eff = hm.data[r * hm.cols + c] + c as f32 * step;
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

// ── NEON parallel west-only ───────────────────────────────────────────────────

#[cfg(target_arch = "aarch64")]
pub unsafe fn compute_shadow_neon_parallel(hm: &Heightmap, sun_elevation_rad: f32) -> ShadowMask {
    use std::arch::aarch64::{
        float32x4_t, uint32x4_t, vaddq_f32, vbslq_f32, vcltq_f32, vdupq_n_f32, vgetq_lane_f32,
        vld1q_f32, vmaxq_f32,
    };
    let mut data: Vec<f32> = vec![1.0f32; hm.rows * hm.cols];
    let dx: f32 = hm.dx_meters as f32;
    let tan_sun: f32 = sun_elevation_rad.tan();
    let step: f32 = dx * tan_sun;

    data.par_chunks_mut(4 * hm.cols)
        .enumerate()
        .for_each(|(chunk_idx, chunk)| {
            let rows_in_chunk = chunk.len() / hm.cols;

            if rows_in_chunk < 4 {
                // scalar remainder chunk
                for local_r in 0..rows_in_chunk {
                    let global_r = chunk_idx * 4 + local_r;
                    let mut running_max = f32::NEG_INFINITY;
                    for c in 0..hm.cols {
                        let h_eff = hm.data[global_r * hm.cols + c] + c as f32 * step;
                        if h_eff < running_max {
                            chunk[local_r * hm.cols + c] = 0.0;
                        }
                        running_max = running_max.max(h_eff);
                    }
                }
                return;
            }

            let r = chunk_idx * 4;
            let base = [
                r * hm.cols,
                (r + 1) * hm.cols,
                (r + 2) * hm.cols,
                (r + 3) * hm.cols,
            ];
            let mut running_max: float32x4_t = unsafe { vdupq_n_f32(f32::NEG_INFINITY) };

            for c in 0..hm.cols {
                let heights = [
                    hm.data[base[0] + c],
                    hm.data[base[1] + c],
                    hm.data[base[2] + c],
                    hm.data[base[3] + c],
                ];
                unsafe {
                    let h_vec = vld1q_f32(heights.as_ptr());
                    let h_eff = vaddq_f32(h_vec, vdupq_n_f32(c as f32 * step));
                    let mask: uint32x4_t = vcltq_f32(h_eff, running_max);
                    let result = vbslq_f32(mask, vdupq_n_f32(0.0), vdupq_n_f32(1.0));

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

// ── NEON parallel arbitrary azimuth (DDA) ────────────────────────────────────
//
// Parallelises over groups of 4 starting pixels (rayon).
// Within each group, 4 rays run simultaneously in NEON lanes.
// When the group's bounding box exits the grid, the surviving rays
// are finished individually with scalar code using the per-lane
// running_max extracted from the NEON register.
//
// Safety: parallel writes to `data` via SendPtr.  Different rays cover
// different pixels for cardinal / near-cardinal azimuths.  For diagonal
// azimuths the two entry-edge sets produce overlapping rays at grid
// corners; those corner pixels may be written by two threads.  On
// AArch64 a 32-bit store is a single instruction — no torn writes.
// The two threads write either 0.0 or 1.0; both are valid f32 values.

#[cfg(target_arch = "aarch64")]
pub unsafe fn compute_shadow_neon_parallel_with_azimuth(
    hm: &Heightmap,
    sun_azimuth_rad: f32,
    sun_elevation_rad: f32,
    penumbra_meters: f32,
) -> ShadowMask {
    use std::arch::aarch64::{
        float32x4_t, vaddq_f32, vdupq_n_f32, vgetq_lane_f32, vld1q_f32, vmaxq_f32, vmulq_n_f32,
        vsubq_f32,
    };
    let inv_penumbra = 1.0 / penumbra_meters;
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

    starting_pixels.par_chunks_mut(4).for_each(|rays| {
        let ptr = data_ptr.get();

        // ── scalar fallback for the last chunk (< 4 rays) ────────────────
        if rays.len() < 4 {
            for (sr, sc) in rays.iter().copied() {
                let mut rm = f32::NEG_INFINITY;
                let mut dist = 0.0f32;
                let (mut rf, mut cf) = (sr, sc);
                while rf >= 0.0 && rf < hm.rows as f32 && cf >= 0.0 && cf < hm.cols as f32 {
                    let r = rf.floor() as usize;
                    let c = cf.floor() as usize;
                    let h_eff = hm.data[r * hm.cols + c] + dist * tan_sun;
                    if h_eff < rm {
                        let margin = rm - h_eff;
                        unsafe {
                            *ptr.add(r * hm.cols + c) = (1.0 - margin / penumbra_meters).max(0.0);
                        }
                    }
                    rm = rm.max(h_eff);
                    rf += dr_step;
                    cf += dc_step;
                    dist += dist_per_step;
                }
            }
            return;
        }

        // ── NEON: 4 rays in parallel ──────────────────────────────────────
        let mut rf = [rays[0].0, rays[1].0, rays[2].0, rays[3].0];
        let mut cf = [rays[0].1, rays[1].1, rays[2].1, rays[3].1];
        let mut dist = 0.0f32;
        let mut running_max: float32x4_t = unsafe { vdupq_n_f32(f32::NEG_INFINITY) };

        // Run NEON while ALL 4 rays are still inside the grid.
        while rf
            .iter()
            .zip(cf.iter())
            .all(|(&r, &c)| r >= 0.0 && r < hm.rows as f32 && c >= 0.0 && c < hm.cols as f32)
        {
            let idxs = [
                // should always be floor, otherwise an edge case with some azimuth
                // (probably direct south e.g.) causes out of bounds panic.
                rf[0].floor() as usize * hm.cols + cf[0].floor() as usize,
                rf[1].floor() as usize * hm.cols + cf[1].floor() as usize,
                rf[2].floor() as usize * hm.cols + cf[2].floor() as usize,
                rf[3].floor() as usize * hm.cols + cf[3].floor() as usize,
            ];
            let heights = [
                hm.data[idxs[0]],
                hm.data[idxs[1]],
                hm.data[idxs[2]],
                hm.data[idxs[3]],
            ];

            unsafe {
                let h_vec = vld1q_f32(heights.as_ptr());
                let h_eff = vaddq_f32(h_vec, vdupq_n_f32(dist * tan_sun));
                let margin = vmaxq_f32(vsubq_f32(running_max, h_eff), vdupq_n_f32(0.0));
                let result = vmaxq_f32(
                    vsubq_f32(vdupq_n_f32(1.0), vmulq_n_f32(margin, inv_penumbra)),
                    vdupq_n_f32(0.0),
                );

                *ptr.add(idxs[0]) = vgetq_lane_f32::<0>(result);
                *ptr.add(idxs[1]) = vgetq_lane_f32::<1>(result);
                *ptr.add(idxs[2]) = vgetq_lane_f32::<2>(result);
                *ptr.add(idxs[3]) = vgetq_lane_f32::<3>(result);

                running_max = vmaxq_f32(running_max, h_eff);
            }

            for i in 0..4 {
                rf[i] += dr_step;
                cf[i] += dc_step;
            }
            dist += dist_per_step;
        }

        // Extract per-lane running_max and finish any rays still in bounds.
        let per_max = unsafe {
            [
                vgetq_lane_f32::<0>(running_max),
                vgetq_lane_f32::<1>(running_max),
                vgetq_lane_f32::<2>(running_max),
                vgetq_lane_f32::<3>(running_max),
            ]
        };

        for i in 0..4 {
            let mut rm = per_max[i];
            let mut r_f = rf[i];
            let mut c_f = cf[i];
            let mut d = dist;
            while r_f >= 0.0 && r_f < hm.rows as f32 && c_f >= 0.0 && c_f < hm.cols as f32 {
                let r = r_f.floor() as usize;
                let c = c_f.floor() as usize;
                let h_eff = hm.data[r * hm.cols + c] + d * tan_sun;
                if h_eff < rm {
                    let margin = rm - h_eff;
                    unsafe {
                        *ptr.add(r * hm.cols + c) = (1.0 - margin / penumbra_meters).max(0.0);
                    }
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
