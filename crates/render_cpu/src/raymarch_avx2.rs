#![cfg(target_arch = "x86_64")]

use crate::Ray;
use dem_io::Heightmap;

// 8-wide AVX2 ray packet (SoA layout, mirroring the 4-wide NEON RayPacket).
pub struct RayPacketAvx2 {
    pub origin_x: core::arch::x86_64::__m256,
    pub origin_y: core::arch::x86_64::__m256,
    pub origin_z: core::arch::x86_64::__m256,
    pub dir_x: core::arch::x86_64::__m256,
    pub dir_y: core::arch::x86_64::__m256,
    pub dir_z: core::arch::x86_64::__m256,
}

impl RayPacketAvx2 {
    #[target_feature(enable = "avx2")]
    pub unsafe fn new(
        r0: &Ray,
        r1: &Ray,
        r2: &Ray,
        r3: &Ray,
        r4: &Ray,
        r5: &Ray,
        r6: &Ray,
        r7: &Ray,
    ) -> Self {
        use core::arch::x86_64::*;
        // _mm256_set_ps(e7, e6, e5, e4, e3, e2, e1, e0): e0 → lane 0, e7 → lane 7
        RayPacketAvx2 {
            origin_x: _mm256_set_ps(
                r7.origin[0], r6.origin[0], r5.origin[0], r4.origin[0],
                r3.origin[0], r2.origin[0], r1.origin[0], r0.origin[0],
            ),
            origin_y: _mm256_set_ps(
                r7.origin[1], r6.origin[1], r5.origin[1], r4.origin[1],
                r3.origin[1], r2.origin[1], r1.origin[1], r0.origin[1],
            ),
            origin_z: _mm256_set_ps(
                r7.origin[2], r6.origin[2], r5.origin[2], r4.origin[2],
                r3.origin[2], r2.origin[2], r1.origin[2], r0.origin[2],
            ),
            dir_x: _mm256_set_ps(
                r7.dir[0], r6.dir[0], r5.dir[0], r4.dir[0],
                r3.dir[0], r2.dir[0], r1.dir[0], r0.dir[0],
            ),
            dir_y: _mm256_set_ps(
                r7.dir[1], r6.dir[1], r5.dir[1], r4.dir[1],
                r3.dir[1], r2.dir[1], r1.dir[1], r0.dir[1],
            ),
            dir_z: _mm256_set_ps(
                r7.dir[2], r6.dir[2], r5.dir[2], r4.dir[2],
                r3.dir[2], r2.dir[2], r1.dir[2], r0.dir[2],
            ),
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub unsafe fn raymarch_avx2(
    packet: &RayPacketAvx2,
    hm: &Heightmap,
    step_m: f32,
    t_max: f32,
) -> [Option<[f32; 3]>; 8] {
    use core::arch::x86_64::*;

    let mut result: [Option<[f32; 3]>; 8] = [None, None, None, None, None, None, None, None];

    // All lanes active: 0xFFFF_FFFF per lane
    let mut is_active: __m256i = _mm256_set1_epi32(-1i32);
    let mut t = 0.0f32;

    let dx = hm.dx_meters as f32;
    let dy = hm.dy_meters as f32;

    while t < t_max {
        let t_vec = _mm256_set1_ps(t);
        let p_x = _mm256_add_ps(packet.origin_x, _mm256_mul_ps(packet.dir_x, t_vec));
        let p_y = _mm256_add_ps(packet.origin_y, _mm256_mul_ps(packet.dir_y, t_vec));
        let p_z = _mm256_add_ps(packet.origin_z, _mm256_mul_ps(packet.dir_z, t_vec));

        let col_f = _mm256_div_ps(p_x, _mm256_set1_ps(dx));
        let row_f = _mm256_div_ps(p_y, _mm256_set1_ps(dy));

        // Out-of-bounds check — deactivate lanes that leave the heightmap.
        // _CMP_LT_OS = 1, _CMP_GE_OS = 13
        let cols_lt_0 =
            _mm256_castps_si256(_mm256_cmp_ps::<1>(col_f, _mm256_setzero_ps()));
        let cols_ge_max =
            _mm256_castps_si256(_mm256_cmp_ps::<13>(col_f, _mm256_set1_ps(hm.cols as f32)));
        let rows_lt_0 =
            _mm256_castps_si256(_mm256_cmp_ps::<1>(row_f, _mm256_setzero_ps()));
        let rows_ge_max =
            _mm256_castps_si256(_mm256_cmp_ps::<13>(row_f, _mm256_set1_ps(hm.rows as f32)));

        let oob = _mm256_or_si256(
            _mm256_or_si256(cols_lt_0, cols_ge_max),
            _mm256_or_si256(rows_lt_0, rows_ge_max),
        );
        // is_active &= !oob
        is_active = _mm256_andnot_si256(oob, is_active);

        // Early exit when all lanes are done.
        if _mm256_testz_si256(is_active, is_active) != 0 {
            return result;
        }

        // Gather 8 terrain heights (scalar gather — same as NEON, just 8 lanes).
        let mut col_arr = [0f32; 8];
        let mut row_arr = [0f32; 8];
        _mm256_storeu_ps(col_arr.as_mut_ptr(), col_f);
        _mm256_storeu_ps(row_arr.as_mut_ptr(), row_f);

        let safe_col = |c: isize| c.clamp(0, hm.cols as isize - 1) as usize;
        let safe_row = |r: isize| r.clamp(0, hm.rows as isize - 1) as usize;

        let terrain_heights = [
            hm.data[safe_row(row_arr[0] as isize) * hm.cols + safe_col(col_arr[0] as isize)],
            hm.data[safe_row(row_arr[1] as isize) * hm.cols + safe_col(col_arr[1] as isize)],
            hm.data[safe_row(row_arr[2] as isize) * hm.cols + safe_col(col_arr[2] as isize)],
            hm.data[safe_row(row_arr[3] as isize) * hm.cols + safe_col(col_arr[3] as isize)],
            hm.data[safe_row(row_arr[4] as isize) * hm.cols + safe_col(col_arr[4] as isize)],
            hm.data[safe_row(row_arr[5] as isize) * hm.cols + safe_col(col_arr[5] as isize)],
            hm.data[safe_row(row_arr[6] as isize) * hm.cols + safe_col(col_arr[6] as isize)],
            hm.data[safe_row(row_arr[7] as isize) * hm.cols + safe_col(col_arr[7] as isize)],
        ];
        let terrain_z = _mm256_loadu_ps(terrain_heights.as_ptr());

        // Hit test: p_z < terrain_z
        let hit_mask_f = _mm256_cmp_ps::<1>(p_z, terrain_z);
        let hit_mask = _mm256_castps_si256(hit_mask_f);
        let new_hits = _mm256_and_si256(hit_mask, is_active);
        // Deactivate hit lanes.
        is_active = _mm256_andnot_si256(new_hits, is_active);

        if _mm256_testz_si256(new_hits, new_hits) == 0 {
            // At least one new hit — binary-search the impact point for all 8 lanes.
            let (rx, ry, rz) = binary_search_hit_avx2(
                packet,
                hm,
                _mm256_set1_ps(t - step_m),
                _mm256_set1_ps(t),
                8,
            );

            let mut rx_arr = [0.0f32; 8];
            let mut ry_arr = [0.0f32; 8];
            let mut rz_arr = [0.0f32; 8];
            _mm256_storeu_ps(rx_arr.as_mut_ptr(), rx);
            _mm256_storeu_ps(ry_arr.as_mut_ptr(), ry);
            _mm256_storeu_ps(rz_arr.as_mut_ptr(), rz);

            let mut hits_arr = [0u32; 8];
            _mm256_storeu_si256(hits_arr.as_mut_ptr() as *mut __m256i, new_hits);

            for i in 0..8 {
                if hits_arr[i] != 0 {
                    result[i] = Some([rx_arr[i], ry_arr[i], rz_arr[i]]);
                }
            }
        }

        t += step_m;
    }

    result
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn binary_search_hit_avx2(
    packet: &RayPacketAvx2,
    hm: &Heightmap,
    mut t_lo: core::arch::x86_64::__m256,
    mut t_hi: core::arch::x86_64::__m256,
    iterations: u32,
) -> (
    core::arch::x86_64::__m256,
    core::arch::x86_64::__m256,
    core::arch::x86_64::__m256,
) {
    use core::arch::x86_64::*;

    for _ in 0..iterations {
        let t_mid = _mm256_mul_ps(_mm256_add_ps(t_lo, t_hi), _mm256_set1_ps(0.5));

        let p_x = _mm256_add_ps(packet.origin_x, _mm256_mul_ps(packet.dir_x, t_mid));
        let p_y = _mm256_add_ps(packet.origin_y, _mm256_mul_ps(packet.dir_y, t_mid));
        let p_z = _mm256_add_ps(packet.origin_z, _mm256_mul_ps(packet.dir_z, t_mid));

        let col_f = _mm256_div_ps(p_x, _mm256_set1_ps(hm.dx_meters as f32));
        let row_f = _mm256_div_ps(p_y, _mm256_set1_ps(hm.dy_meters as f32));

        let mut col_arr = [0f32; 8];
        let mut row_arr = [0f32; 8];
        _mm256_storeu_ps(col_arr.as_mut_ptr(), col_f);
        _mm256_storeu_ps(row_arr.as_mut_ptr(), row_f);

        let safe_col = |c: isize| c.clamp(0, hm.cols as isize - 1) as usize;
        let safe_row = |r: isize| r.clamp(0, hm.rows as isize - 1) as usize;

        let terrain_heights = [
            hm.data[safe_row(row_arr[0] as isize) * hm.cols + safe_col(col_arr[0] as isize)],
            hm.data[safe_row(row_arr[1] as isize) * hm.cols + safe_col(col_arr[1] as isize)],
            hm.data[safe_row(row_arr[2] as isize) * hm.cols + safe_col(col_arr[2] as isize)],
            hm.data[safe_row(row_arr[3] as isize) * hm.cols + safe_col(col_arr[3] as isize)],
            hm.data[safe_row(row_arr[4] as isize) * hm.cols + safe_col(col_arr[4] as isize)],
            hm.data[safe_row(row_arr[5] as isize) * hm.cols + safe_col(col_arr[5] as isize)],
            hm.data[safe_row(row_arr[6] as isize) * hm.cols + safe_col(col_arr[6] as isize)],
            hm.data[safe_row(row_arr[7] as isize) * hm.cols + safe_col(col_arr[7] as isize)],
        ];
        let terrain_z = _mm256_loadu_ps(terrain_heights.as_ptr());

        // below = p_z < terrain_z (float mask)
        let below = _mm256_cmp_ps::<1>(p_z, terrain_z);
        // If below: t_hi = t_mid, else t_hi stays
        t_hi = _mm256_blendv_ps(t_hi, t_mid, below);
        // If below: t_lo stays, else t_lo = t_mid
        t_lo = _mm256_blendv_ps(t_mid, t_lo, below);
    }

    let p_x = _mm256_add_ps(packet.origin_x, _mm256_mul_ps(packet.dir_x, t_lo));
    let p_y = _mm256_add_ps(packet.origin_y, _mm256_mul_ps(packet.dir_y, t_lo));
    let p_z = _mm256_add_ps(packet.origin_z, _mm256_mul_ps(packet.dir_z, t_lo));
    (p_x, p_y, p_z)
}
