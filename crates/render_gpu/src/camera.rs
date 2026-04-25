use dem_io::Heightmap;

use crate::vector_utils::*;

// GPU-ready camera data. Must match the WGSL struct byte-for-byte.
// repr(C) + Pod guarantees bytemuck can cast it to &[u8] for upload.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CameraUniforms {
    pub origin: [f32; 3],
    pub _pad0: f32, // vec3 in WGSL is 16-byte aligned
    pub forward: [f32; 3],
    pub _pad1: f32,
    pub right: [f32; 3],
    pub _pad2: f32,
    pub up: [f32; 3],
    pub _pad3: f32,
    pub sun_dir: [f32; 3],
    pub _pad4: f32,
    pub half_w: f32,
    pub half_h: f32,
    pub img_width: u32,
    pub img_height: u32,
    pub hm_cols: u32,
    pub hm_rows: u32,
    pub dx_meters: f32,
    pub dy_meters: f32,
    pub step_m: f32,
    pub t_max: f32,
    pub _pad5: f32, // pad to 16-byte boundary
    pub _pad6: f32,
    pub _pad7: f32,
    pub _pad8: f32,
    pub _pad9: f32,
    pub _pad10: f32,
}

impl CameraUniforms {
    pub fn new(
        origin: [f32; 3],
        look_at: [f32; 3],
        fov_deg: f32,
        aspect: f32,
        hm: &Heightmap,
        sun_dir: [f32; 3],
        img_width: u32,
        img_height: u32,
        step_m: f32,
        t_max: f32,
    ) -> CameraUniforms {
        let forward: [f32; 3] = normalize(sub(look_at, origin));
        let right: [f32; 3] = normalize(cross(forward, [0.0, 0.0, 1.0]));
        // let right = normalize(cross([0.0, 0.0, 1.0], forward)); // reversed cross
        let up: [f32; 3] = cross(right, forward);
        let half_w: f32 = (fov_deg / 2.0).to_radians().tan();
        let half_h: f32 = half_w / aspect;

        CameraUniforms {
            origin,
            _pad0: 0.0,
            forward,
            _pad1: 0.0,
            right,
            _pad2: 0.0,
            up,
            _pad3: 0.0,
            sun_dir,
            _pad4: 0.0,
            half_w,
            half_h,
            img_width,
            img_height,
            hm_cols: hm.cols as u32,
            hm_rows: hm.rows as u32,
            dx_meters: hm.dx_meters as f32,
            dy_meters: hm.dy_meters as f32,
            step_m,
            t_max,
            _pad5: 0.0,
            _pad6: 0.0,
            _pad7: 0.0,
            _pad8: 0.0,
            _pad9: 0.0,
            _pad10: 0.0,
        }
    }
}
