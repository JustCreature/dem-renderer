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

/// World-space camera basis `(forward, right, up)` for a +Z-up look-at camera.
/// Shared by every `CameraUniforms` build site so the basis math lives in one place.
pub(crate) fn camera_basis(origin: [f32; 3], look_at: [f32; 3]) -> ([f32; 3], [f32; 3], [f32; 3]) {
    let forward = normalize(sub(look_at, origin));
    let right = normalize(cross(forward, [0.0, 0.0, 1.0]));
    let up = cross(right, forward);
    (forward, right, up)
}

/// Near-plane half-extents `(half_w, half_h)` from horizontal fov and aspect ratio.
pub(crate) fn projection_half_extents(fov_deg: f32, aspect: f32) -> (f32, f32) {
    let half_w = (fov_deg / 2.0).to_radians().tan();
    (half_w, half_w / aspect)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }
    fn len(a: [f32; 3]) -> f32 {
        dot(a, a).sqrt()
    }

    /// Layout guard: the struct is mirrored byte-for-byte in WGSL (std140). Any
    /// field added/removed/reordered without updating the shader must trip this.
    /// Total: 14 vec4-sized rows × 16 bytes = 224.
    ///
    /// The size check alone would pass a same-size field swap, so we also pin the
    /// offsets of the std140-sensitive `vec3` members — each must sit on a 16-byte
    /// boundary, which is exactly what the trailing `_padN` fields exist to enforce.
    /// A missing pad shifts one of these and fails here instead of silently making
    /// the shader read the wrong bytes.
    #[test]
    fn camera_uniforms_layout_is_std140() {
        assert_eq!(std::mem::size_of::<CameraUniforms>(), 224);
        assert_eq!(std::mem::offset_of!(CameraUniforms, origin), 0);
        assert_eq!(std::mem::offset_of!(CameraUniforms, forward), 16);
        assert_eq!(std::mem::offset_of!(CameraUniforms, right), 32);
        assert_eq!(std::mem::offset_of!(CameraUniforms, up), 48);
        assert_eq!(std::mem::offset_of!(CameraUniforms, sun_dir), 64);
    }

    #[test]
    fn camera_basis_is_orthonormal() {
        let (forward, right, up) = camera_basis(
            [0.0, 0.0, 1000.0], // origin
            [100.0, 50.0, 0.0], // look_at (not parallel to world up)
        );

        let eps = 1e-5;
        assert!((len(forward) - 1.0).abs() < eps, "forward not unit");
        assert!((len(right) - 1.0).abs() < eps, "right not unit");
        assert!((len(up) - 1.0).abs() < eps, "up not unit");
        assert!(dot(forward, right).abs() < eps, "forward·right ≠ 0");
        assert!(dot(forward, up).abs() < eps, "forward·up ≠ 0");
        assert!(dot(right, up).abs() < eps, "right·up ≠ 0");
    }

    #[test]
    fn projection_half_extents_scale_with_aspect() {
        let aspect = 2.0;
        let (half_w, half_h) = projection_half_extents(90.0, aspect);
        // 90° horizontal fov → half_w = tan(45°) = 1.
        assert!((half_w - 1.0).abs() < 1e-6, "half_w");
        assert!(
            (half_h - half_w / aspect).abs() < 1e-6,
            "half_h tracks aspect"
        );
    }
}
