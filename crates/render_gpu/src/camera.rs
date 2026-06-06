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
    pub ao_mode: u32,
    pub _pad5: f32, // pad to 16-byte boundary
    pub shadows_enabled: u32,
    pub fog_enabled: u32,
    pub vat_mode: u32,
    pub lod_mode: u32,
    // 5m close tier (extent_x == 0.0 means inactive)
    pub hm5m_origin_x: f32,
    pub hm5m_origin_y: f32,
    pub hm5m_extent_x: f32,
    pub hm5m_extent_y: f32,
    pub hm5m_cols: u32,
    pub hm5m_rows: u32,
    pub hm5m_cos_rot: f32, // cos(align_rot) for 5m tier; default 1.0
    pub hm5m_sin_rot: f32, // sin(align_rot) for 5m tier; default 0.0
    // 1m fine tier (extent_x == 0.0 means inactive)
    pub hm1m_origin_x: f32,
    pub hm1m_origin_y: f32,
    pub hm1m_extent_x: f32,
    pub hm1m_extent_y: f32,
    pub hm1m_cols: u32,
    pub hm1m_rows: u32,
    pub hm1m_cos_rot: f32, // cos(align_rot) for 1m tier; default 1.0
    pub hm1m_sin_rot: f32, // sin(align_rot) for 1m tier; default 0.0
    pub max_terrain_h: f32,
    pub smooth_radius_m: f32,
    pub align_mode: u32, // 0=off, 1=tier viz (green/blue/red)
    pub _pad7: f32,
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
        ao_mode: u32,
        shadows_enabled: u32,
        fog_enabled: u32,
        vat_mode: u32,
        lod_mode: u32,
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
            ao_mode,
            _pad5: 0.0,
            shadows_enabled,
            fog_enabled,
            vat_mode,
            lod_mode,
            hm5m_origin_x: 0.0,
            hm5m_origin_y: 0.0,
            hm5m_extent_x: 0.0,
            hm5m_extent_y: 0.0,
            hm5m_cols: 0,
            hm5m_rows: 0,
            hm5m_cos_rot: 1.0,
            hm5m_sin_rot: 0.0,
            hm1m_origin_x: 0.0,
            hm1m_origin_y: 0.0,
            hm1m_extent_x: 0.0,
            hm1m_extent_y: 0.0,
            hm1m_cols: 0,
            hm1m_rows: 0,
            hm1m_cos_rot: 1.0,
            hm1m_sin_rot: 0.0,
            max_terrain_h: hm.data.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
            smooth_radius_m: 2000.0,
            align_mode: 0,
            _pad7: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal heightmap fixture. `CameraUniforms::new` only reads `cols`, `rows`,
    /// `dx_meters`, `dy_meters` and `data` (for `max_terrain_h`); the geo/CRS
    /// fields are neutral placeholders.
    fn hm(rows: usize, cols: usize, data: Vec<f32>) -> Heightmap {
        Heightmap {
            data,
            rows,
            cols,
            nodata: -9999.0,
            origin_lat: 0.0,
            origin_lon: 0.0,
            dx_deg: 0.0,
            dy_deg: 0.0,
            dx_meters: 5.0,
            dy_meters: 5.0,
            crs_origin_x: 0.0,
            crs_origin_y: 0.0,
            crs_epsg: 0,
            crs_proj4: String::new(),
        }
    }

    fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }
    fn len(a: [f32; 3]) -> f32 {
        dot(a, a).sqrt()
    }

    /// Layout guard: the struct is mirrored byte-for-byte in WGSL (std140). Any
    /// field added/removed without updating the shader must trip this. 14 vec4-
    /// sized rows × 16 bytes = 224.
    #[test]
    fn camera_uniforms_layout_is_std140() {
        assert_eq!(std::mem::size_of::<CameraUniforms>(), 224);
        assert_eq!(std::mem::size_of::<CameraUniforms>() % 16, 0);
    }

    #[test]
    fn basis_is_orthonormal() {
        let h = hm(4, 4, vec![0.0; 16]);
        let cam = CameraUniforms::new(
            [0.0, 0.0, 1000.0], // origin
            [100.0, 50.0, 0.0], // look_at (not parallel to world up)
            60.0,               // fov
            16.0 / 9.0,         // aspect
            &h,
            [0.0, 0.0, 1.0], // sun_dir
            1920,
            1080,
            10.0,
            5000.0,
            0,
            1,
            1,
            0,
            0,
        );

        let eps = 1e-5;
        assert!((len(cam.forward) - 1.0).abs() < eps, "forward not unit");
        assert!((len(cam.right) - 1.0).abs() < eps, "right not unit");
        assert!((len(cam.up) - 1.0).abs() < eps, "up not unit");
        assert!(dot(cam.forward, cam.right).abs() < eps, "forward·right ≠ 0");
        assert!(dot(cam.forward, cam.up).abs() < eps, "forward·up ≠ 0");
        assert!(dot(cam.right, cam.up).abs() < eps, "right·up ≠ 0");
    }

    #[test]
    fn projection_and_dims_propagate() {
        let h = hm(8, 6, vec![0.0; 48]);
        let aspect = 2.0;
        let cam = CameraUniforms::new(
            [0.0, 0.0, 500.0],
            [1.0, 0.0, 0.0],
            90.0,
            aspect,
            &h,
            [0.0, 0.0, 1.0],
            800,
            400,
            10.0,
            5000.0,
            2,
            1,
            0,
            0,
            1,
        );

        assert!((cam.half_h - cam.half_w / aspect).abs() < 1e-6);
        assert_eq!((cam.hm_cols, cam.hm_rows), (6, 8));
        assert_eq!(cam.dx_meters, 5.0);
        assert_eq!((cam.img_width, cam.img_height), (800, 400));
        assert_eq!((cam.ao_mode, cam.lod_mode), (2, 1));
    }

    #[test]
    fn tiers_default_inactive_and_max_height_from_data() {
        let h = hm(3, 3, vec![1.0, 7.0, 2.0, 3.0, 4.0, 5.0, 6.0, 0.0, 1.0]);
        let cam = CameraUniforms::new(
            [0.0, 0.0, 100.0],
            [1.0, 0.0, 0.0],
            60.0,
            1.0,
            &h,
            [0.0, 0.0, 1.0],
            100,
            100,
            10.0,
            5000.0,
            0,
            0,
            0,
            0,
            0,
        );

        // Detail tiers start inactive (extent_x == 0.0) with identity rotation.
        assert_eq!(cam.hm5m_extent_x, 0.0);
        assert_eq!(cam.hm1m_extent_x, 0.0);
        assert_eq!((cam.hm5m_cos_rot, cam.hm5m_sin_rot), (1.0, 0.0));
        assert_eq!((cam.hm1m_cos_rot, cam.hm1m_sin_rot), (1.0, 0.0));
        // max over the data.
        assert_eq!(cam.max_terrain_h, 7.0);
        assert_eq!(cam.smooth_radius_m, 2000.0);
    }
}
