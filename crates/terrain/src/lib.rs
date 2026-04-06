mod row_major;
mod shadow;
mod tiled;

pub use row_major::compute_normals_scalar;
#[cfg(target_arch = "aarch64")]
pub use row_major::{compute_normals_neon, compute_normals_neon_8, compute_normals_neon_parallel};
pub use shadow::{
    ShadowMask, compute_shadow_scalar, compute_shadow_scalar_branchless,
    compute_shadow_scalar_with_azimuth,
};
#[cfg(target_arch = "aarch64")]
pub use shadow::{
    compute_shadow_neon, compute_shadow_neon_parallel, compute_shadow_neon_parallel_with_azimuth,
};
#[cfg(target_arch = "aarch64")]
pub use tiled::{compute_normals_neon_tiled, compute_normals_neon_tiled_parallel};

pub fn compute_normals_vector(hm: &dem_io::Heightmap) -> NormalMap {
    #[cfg(target_arch = "aarch64")]
    return unsafe { row_major::compute_normals_neon(hm) };
    // TODO: replace with compute_normals_avx2 once implemented
    #[cfg(not(target_arch = "aarch64"))]
    return row_major::compute_normals_scalar(hm);
}

pub fn compute_normals_vector_par(hm: &dem_io::Heightmap) -> NormalMap {
    #[cfg(target_arch = "aarch64")]
    return unsafe { row_major::compute_normals_neon_parallel(hm) };
    // TODO: replace with compute_normals_avx2_parallel once implemented
    #[cfg(not(target_arch = "aarch64"))]
    return row_major::compute_normals_scalar(hm);
}

pub fn compute_normals_vector_tiled(tiled_hm: &dem_io::TiledHeightmap) -> NormalMap {
    #[cfg(target_arch = "aarch64")]
    return unsafe { tiled::compute_normals_neon_tiled(tiled_hm) };
    // TODO: implement compute_normals_avx2_tiled
    #[cfg(not(target_arch = "aarch64"))]
    unimplemented!("compute_normals_vector_tiled: no x86 implementation yet");
}

pub fn compute_normals_vector_tiled_par(tiled_hm: &dem_io::TiledHeightmap) -> NormalMap {
    #[cfg(target_arch = "aarch64")]
    return unsafe { tiled::compute_normals_neon_tiled_parallel(tiled_hm) };
    // TODO: implement compute_normals_avx2_tiled_parallel
    #[cfg(not(target_arch = "aarch64"))]
    unimplemented!("compute_normals_vector_tiled_par: no x86 implementation yet");
}

pub fn compute_shadow_vector(hm: &dem_io::Heightmap, sun_elevation_rad: f32) -> ShadowMask {
    #[cfg(target_arch = "aarch64")]
    return unsafe { shadow::compute_shadow_neon(hm, sun_elevation_rad) };
    // TODO: replace with compute_shadow_avx2 once implemented
    #[cfg(not(target_arch = "aarch64"))]
    return shadow::compute_shadow_scalar(hm, sun_elevation_rad);
}

pub fn compute_shadow_vector_par(hm: &dem_io::Heightmap, sun_elevation_rad: f32) -> ShadowMask {
    #[cfg(target_arch = "aarch64")]
    return unsafe { shadow::compute_shadow_neon_parallel(hm, sun_elevation_rad) };
    // TODO: replace with compute_shadow_avx2_parallel once implemented
    #[cfg(not(target_arch = "aarch64"))]
    return shadow::compute_shadow_scalar(hm, sun_elevation_rad);
}

pub fn compute_shadow_vector_par_with_azimuth(
    hm: &dem_io::Heightmap,
    sun_azimuth_rad: f32,
    sun_elevation_rad: f32,
) -> ShadowMask {
    #[cfg(target_arch = "aarch64")]
    return unsafe {
        shadow::compute_shadow_neon_parallel_with_azimuth(hm, sun_azimuth_rad, sun_elevation_rad)
    };
    // TODO: replace with compute_shadow_avx2_parallel_with_azimuth once implemented
    #[cfg(not(target_arch = "aarch64"))]
    return shadow::compute_shadow_scalar_with_azimuth(hm, sun_azimuth_rad, sun_elevation_rad);
}

pub(crate) struct SendPtr(*mut f32);
unsafe impl Send for SendPtr {}
unsafe impl Sync for SendPtr {}

impl SendPtr {
    fn get(&self) -> *mut f32 {
        // Why this happens in Rust 2021/2024: the edition changed closure capture to use
        // "precise disjoint capture" — closures capture the minimal path they access.
        // nx_ptr.0 is a field path of type *mut f32, so that's what gets
        // captured, bypassing the Send + Sync impls on SendPtr.
        // Using a method call forces the closure to capture nx_ptr (the whole struct) rather than its inner field.
        self.0
    }
}

pub struct NormalMap {
    pub nx: Vec<f32>,
    pub ny: Vec<f32>,
    pub nz: Vec<f32>,
    pub rows: usize,
    pub cols: usize,
}
