use crate::vector_utils::*;
use core::arch::aarch64::*;
use std::u32;

use crate::Ray;
use dem_io::Heightmap;

pub struct RayPacket {
    pub origin_x: float32x4_t,
    pub origin_y: float32x4_t,
    pub origin_z: float32x4_t,
    pub dir_x: float32x4_t,
    pub dir_y: float32x4_t,
    pub dir_z: float32x4_t,
}

impl RayPacket {
    pub fn new(r0: &Ray, r1: &Ray, r2: &Ray, r3: &Ray) -> Self {
        unsafe {
            RayPacket {
                origin_x: vld1q_f32(
                    [r0.origin[0], r1.origin[0], r2.origin[0], r3.origin[0]].as_ptr(),
                ),
                origin_y: vld1q_f32(
                    [r0.origin[1], r1.origin[1], r2.origin[1], r3.origin[1]].as_ptr(),
                ),
                origin_z: vld1q_f32(
                    [r0.origin[2], r1.origin[2], r2.origin[2], r3.origin[2]].as_ptr(),
                ),
                dir_x: vld1q_f32([r0.dir[0], r1.dir[0], r2.dir[0], r3.dir[0]].as_ptr()),
                dir_y: vld1q_f32([r0.dir[1], r1.dir[1], r2.dir[1], r3.dir[1]].as_ptr()),
                dir_z: vld1q_f32([r0.dir[2], r1.dir[2], r2.dir[2], r3.dir[2]].as_ptr()),
            }
        }
    }
}

pub unsafe fn raymarch_neon(
    packet: &RayPacket,
    hm: &Heightmap,
    step_m: f32,
    t_max: f32,
) -> [Option<[f32; 3]>; 4] {
    let mut result: [Option<[f32; 3]>; 4] = [None, None, None, None];

    unsafe {
        let mut is_active: uint32x4_t = vdupq_n_u32(u32::MAX);
        let mut t: f32 = 0.0f32;

        let dx: f32 = hm.dx_meters as f32;
        let dy: f32 = hm.dy_meters as f32;

        while t < t_max {
            // 1. Current position in worlkd space
            // p = origin + t * dir start at 0, from camera
            let t_vec: float32x4_t = vdupq_n_f32(t);
            let p_x: float32x4_t = vaddq_f32(packet.origin_x, vmulq_f32(packet.dir_x, t_vec));
            let p_y: float32x4_t = vaddq_f32(packet.origin_y, vmulq_f32(packet.dir_y, t_vec));
            let p_z: float32x4_t = vaddq_f32(packet.origin_z, vmulq_f32(packet.dir_z, t_vec));

            // 2. convert heitmap to pixel indices
            let col_f: float32x4_t = vdivq_f32(p_x, vdupq_n_f32(dx));
            let row_f: float32x4_t = vdivq_f32(p_y, vdupq_n_f32(dy));

            // 3. bounds check - ray left the heightmap, no hit
            // extract lanes for bounds check + gather
            let cols = [
                vgetq_lane_f32(col_f, 0) as isize,
                vgetq_lane_f32(col_f, 1) as isize,
                vgetq_lane_f32(col_f, 2) as isize,
                vgetq_lane_f32(col_f, 3) as isize,
            ];
            let rows = [
                vgetq_lane_f32(row_f, 0) as isize,
                vgetq_lane_f32(row_f, 1) as isize,
                vgetq_lane_f32(row_f, 2) as isize,
                vgetq_lane_f32(row_f, 3) as isize,
            ];
            let cols_lt_0: uint32x4_t = vcltq_f32(col_f, vdupq_n_f32(0.0));
            let cols_ge_max: uint32x4_t = vcgeq_f32(col_f, vdupq_n_f32(hm.cols as f32));
            let rows_lt_0: uint32x4_t = vcltq_f32(row_f, vdupq_n_f32(0.0));
            let rows_ge_max: uint32x4_t = vcgeq_f32(row_f, vdupq_n_f32(hm.rows as f32));

            let oob = vorrq_u32(
                vorrq_u32(cols_lt_0, cols_ge_max),
                vorrq_u32(rows_lt_0, rows_ge_max),
            );
            is_active = vbicq_u32(is_active, oob);

            if vmaxvq_u32(is_active) == 0 {
                return result;
            }

            // 4. sample terrain height
            // gather terrain heights
            let safe_col = |c: isize| c.clamp(0, hm.cols as isize - 1);
            let safe_row = |r: isize| r.clamp(0, hm.rows as isize - 1);

            let t0: f32 =
                hm.data[safe_row(rows[0]) as usize * hm.cols + safe_col(cols[0]) as usize] as f32;
            let t1: f32 =
                hm.data[safe_row(rows[1]) as usize * hm.cols + safe_col(cols[1]) as usize] as f32;
            let t2: f32 =
                hm.data[safe_row(rows[2]) as usize * hm.cols + safe_col(cols[2]) as usize] as f32;
            let t3: f32 =
                hm.data[safe_row(rows[3]) as usize * hm.cols + safe_col(cols[3]) as usize] as f32;
            let terrain_z: float32x4_t = vld1q_f32([t0, t1, t2, t3].as_ptr());

            // 5. hit test + update active mask + store results
            let hit_mask = vcltq_f32(p_z, terrain_z); // p_z < terrain_z                                                                              
            let new_hits = vandq_u32(hit_mask, is_active); // only active lanes                                                                     
            is_active = vbicq_u32(is_active, new_hits); // deactivate hit lanes

            if vmaxvq_u32(new_hits) != 0 {
                let (rx, ry, rz) =
                    binary_search_hit_neon(packet, hm, vdupq_n_f32(t - step_m), vdupq_n_f32(t), 8);

                if vgetq_lane_u32::<0>(new_hits) != 0 {
                    result[0] = Some([
                        vgetq_lane_f32::<0>(rx),
                        vgetq_lane_f32::<0>(ry),
                        vgetq_lane_f32::<0>(rz),
                    ]);
                }
                if vgetq_lane_u32::<1>(new_hits) != 0 {
                    result[1] = Some([
                        vgetq_lane_f32::<1>(rx),
                        vgetq_lane_f32::<1>(ry),
                        vgetq_lane_f32::<1>(rz),
                    ]);
                }
                if vgetq_lane_u32::<2>(new_hits) != 0 {
                    result[2] = Some([
                        vgetq_lane_f32::<2>(rx),
                        vgetq_lane_f32::<2>(ry),
                        vgetq_lane_f32::<2>(rz),
                    ]);
                }
                if vgetq_lane_u32::<3>(new_hits) != 0 {
                    result[3] = Some([
                        vgetq_lane_f32::<3>(rx),
                        vgetq_lane_f32::<3>(ry),
                        vgetq_lane_f32::<3>(rz),
                    ]);
                }
            }

            t += step_m;
        }
    }

    result
}

unsafe fn binary_search_hit_neon(
    packet: &RayPacket,
    hm: &Heightmap,
    mut t_lo: float32x4_t,
    mut t_hi: float32x4_t,
    iterations: u32,
) -> (float32x4_t, float32x4_t, float32x4_t) {
    for _ in 0..iterations {
        let t_mid = vmulq_f32(vaddq_f32(t_lo, t_hi), vdupq_n_f32(0.5));

        // compute p for all 4 lanes at t_mid
        let p_x = vaddq_f32(packet.origin_x, vmulq_f32(packet.dir_x, t_mid));
        let p_y = vaddq_f32(packet.origin_y, vmulq_f32(packet.dir_y, t_mid));
        let p_z = vaddq_f32(packet.origin_z, vmulq_f32(packet.dir_z, t_mid));

        // gather terrain heights (same pattern as main loop)
        let col_f = vdivq_f32(p_x, vdupq_n_f32(hm.dx_meters as f32));
        let row_f = vdivq_f32(p_y, vdupq_n_f32(hm.dy_meters as f32));

        let safe_col = |c: isize| c.clamp(0, hm.cols as isize - 1);
        let safe_row = |r: isize| r.clamp(0, hm.rows as isize - 1);

        let cols = [
            vgetq_lane_f32(col_f, 0) as isize,
            vgetq_lane_f32(col_f, 1) as isize,
            vgetq_lane_f32(col_f, 2) as isize,
            vgetq_lane_f32(col_f, 3) as isize,
        ];
        let rows = [
            vgetq_lane_f32(row_f, 0) as isize,
            vgetq_lane_f32(row_f, 1) as isize,
            vgetq_lane_f32(row_f, 2) as isize,
            vgetq_lane_f32(row_f, 3) as isize,
        ];

        let h0 = hm.data[safe_row(rows[0]) as usize * hm.cols + safe_col(cols[0]) as usize] as f32;
        let h1 = hm.data[safe_row(rows[1]) as usize * hm.cols + safe_col(cols[1]) as usize] as f32;
        let h2 = hm.data[safe_row(rows[2]) as usize * hm.cols + safe_col(cols[2]) as usize] as f32;
        let h3 = hm.data[safe_row(rows[3]) as usize * hm.cols + safe_col(cols[3]) as usize] as f32;
        let terrain_z = vld1q_f32([h0, h1, h2, h3].as_ptr());

        // if p_z < terrain_z → below → t_hi = t_mid, else t_lo = t_mid
        let below = vcltq_f32(p_z, terrain_z);
        t_hi = vbslq_f32(below, t_mid, t_hi); // if below: t_hi=t_mid, else keep t_hi
        t_lo = vbslq_f32(below, t_lo, t_mid); // if below: keep t_lo, else t_lo=t_mid                                                                             
    }

    let p_x = vaddq_f32(packet.origin_x, vmulq_f32(packet.dir_x, t_lo));
    let p_y = vaddq_f32(packet.origin_y, vmulq_f32(packet.dir_y, t_lo));
    let p_z = vaddq_f32(packet.origin_z, vmulq_f32(packet.dir_z, t_lo));
    (p_x, p_y, p_z)
}
