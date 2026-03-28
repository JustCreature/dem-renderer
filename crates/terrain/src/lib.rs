pub struct NormalMap {
    pub nx: Vec<f32>,
    pub ny: Vec<f32>,
    pub nz: Vec<f32>,
    pub rows: usize,
    pub cols: usize,
}

pub fn compute_normals_scalar(hm: &dem_io::Heightmap) -> NormalMap {
    let mut nx: Vec<f32> = vec![0.0f32; hm.rows * hm.cols];
    let mut ny: Vec<f32> = vec![0.0f32; hm.rows * hm.cols];
    let mut nz: Vec<f32> = vec![0.0f32; hm.rows * hm.cols];

    for r in 1..hm.rows - 1 {
        for c in 1..hm.cols - 1 {
            let upper: f32 = hm.data[(r - 1) * hm.cols + c] as f32;
            let lower: f32 = hm.data[(r + 1) * hm.cols + c] as f32;
            let left: f32 = hm.data[r * hm.cols + (c - 1)] as f32;
            let right: f32 = hm.data[r * hm.cols + (c + 1)] as f32;

            let single_nx: f32 = (left - right) / (2.0 * hm.dx_meters) as f32;
            let single_ny: f32 = (upper - lower) / (2.0 * hm.dy_meters) as f32;
            let single_nz: f32 = 1.0;

            let length: f32 =
                f32::sqrt(single_nx * single_nx + single_ny * single_ny + single_nz * single_nz);

            nx[r * hm.cols + c] = single_nx / length;
            ny[r * hm.cols + c] = single_ny / length;
            nz[r * hm.cols + c] = single_nz / length;
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
pub unsafe fn compute_normals_neon(hm: &dem_io::Heightmap) -> NormalMap {
    use core::arch::aarch64::*;

    let mut nx: Vec<f32> = vec![0.0f32; hm.rows * hm.cols];
    let mut ny: Vec<f32> = vec![0.0f32; hm.rows * hm.cols];
    let mut nz: Vec<f32> = vec![0.0f32; hm.rows * hm.cols];

    unsafe {
        let inv_2dx: float32x4_t = vdupq_n_f32(1.0 / (2.0 * hm.dx_meters as f32));
        let inv_2dy: float32x4_t = vdupq_n_f32(1.0 / (2.0 * hm.dy_meters as f32));

        for r in 1..hm.rows - 1 {
            let mut c = 1usize;
            while c + 4 < hm.cols - 1 {
                let ptr_upper = hm.data.as_ptr().add((r - 1) * hm.cols + c);
                let upper = vcvtq_f32_s32(vmovl_s16(vld1_s16(ptr_upper)));

                let ptr_lower = hm.data.as_ptr().add((r + 1) * hm.cols + c);
                let lower = vcvtq_f32_s32(vmovl_s16(vld1_s16(ptr_lower)));

                let ptr_left = hm.data.as_ptr().add(r * hm.cols + (c - 1));
                let left = vcvtq_f32_s32(vmovl_s16(vld1_s16(ptr_left)));

                let ptr_right = hm.data.as_ptr().add(r * hm.cols + (c + 1));
                let right = vcvtq_f32_s32(vmovl_s16(vld1_s16(ptr_right)));

                let vec_nx: float32x4_t = vmulq_f32(vsubq_f32(left, right), inv_2dx);
                let vec_ny: float32x4_t = vmulq_f32(vsubq_f32(upper, lower), inv_2dy);
                let vec_nz = vdupq_n_f32(1.0);

                let len_sq: float32x4_t = vaddq_f32(
                    vaddq_f32(vmulq_f32(vec_nx, vec_nx), vmulq_f32(vec_ny, vec_ny)),
                    vmulq_f32(vec_nz, vec_nz),
                );
                let reciprocal_sqrt_est: float32x4_t = vrsqrteq_f32(len_sq);
                let refined_sqrt = vmulq_f32(
                    vrsqrtsq_f32(vmulq_f32(len_sq, reciprocal_sqrt_est), reciprocal_sqrt_est),
                    reciprocal_sqrt_est,
                );

                vst1q_f32(
                    nx.as_mut_ptr().add(r * hm.cols + c),
                    vmulq_f32(vec_nx, refined_sqrt),
                );
                vst1q_f32(
                    ny.as_mut_ptr().add(r * hm.cols + c),
                    vmulq_f32(vec_ny, refined_sqrt),
                );
                vst1q_f32(
                    nz.as_mut_ptr().add(r * hm.cols + c),
                    vmulq_f32(vec_nz, refined_sqrt),
                );

                c += 4;
            }
            // scalar tail: c..cols-1
            while c < hm.cols - 1 {
                let upper: f32 = hm.data[(r - 1) * hm.cols + c] as f32;
                let lower: f32 = hm.data[(r + 1) * hm.cols + c] as f32;
                let left: f32 = hm.data[r * hm.cols + (c - 1)] as f32;
                let right: f32 = hm.data[r * hm.cols + (c + 1)] as f32;

                let single_nx: f32 = (left - right) / (2.0 * hm.dx_meters) as f32;
                let single_ny: f32 = (upper - lower) / (2.0 * hm.dy_meters) as f32;
                let single_nz: f32 = 1.0;

                let length: f32 = f32::sqrt(
                    single_nx * single_nx + single_ny * single_ny + single_nz * single_nz,
                );

                nx[r * hm.cols + c] = single_nx / length;
                ny[r * hm.cols + c] = single_ny / length;
                nz[r * hm.cols + c] = single_nz / length;

                c += 1
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
