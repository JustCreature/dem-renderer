mod row_major;
#[cfg(target_arch = "x86_64")]
mod row_major_avx2;
mod shadow;
#[cfg(target_arch = "x86_64")]
mod shadow_avx2;
mod tiled;
#[cfg(target_arch = "x86_64")]
mod tiled_avx2;

use std::usize;

use dem_io::Heightmap;
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

#[cfg(target_arch = "x86_64")]
pub use row_major_avx2::{compute_normals_avx2, compute_normals_avx2_parallel};
#[cfg(target_arch = "x86_64")]
pub use shadow_avx2::{
    compute_shadow_avx2, compute_shadow_avx2_parallel, compute_shadow_avx2_parallel_with_azimuth,
};
#[cfg(target_arch = "x86_64")]
pub use tiled_avx2::{compute_normals_avx2_tiled, compute_normals_avx2_tiled_parallel};

// ── Platform dispatchers ──────────────────────────────────────────────────────
// On aarch64 → NEON (always available).
// On x86_64  → AVX2 when detected at runtime, scalar fallback otherwise.
// Other platforms → scalar only.

pub fn compute_normals_vector(hm: &dem_io::Heightmap) -> NormalMap {
    #[cfg(target_arch = "aarch64")]
    return unsafe { row_major::compute_normals_neon(hm) };

    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx2") {
        return unsafe { row_major_avx2::compute_normals_avx2(hm) };
    }

    #[cfg(not(target_arch = "aarch64"))]
    {
        #[cfg(target_arch = "x86_64")]
        eprintln!("[SCALAR FALLBACK] compute_normals_vector: AVX2 not detected");
        #[cfg(not(target_arch = "x86_64"))]
        eprintln!("[SCALAR FALLBACK] compute_normals_vector: no SIMD for this architecture");
        return row_major::compute_normals_scalar(hm);
    }
}

pub fn compute_normals_vector_par(hm: &dem_io::Heightmap) -> NormalMap {
    #[cfg(target_arch = "aarch64")]
    return unsafe { row_major::compute_normals_neon_parallel(hm) };

    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx2") {
        return unsafe { row_major_avx2::compute_normals_avx2_parallel(hm) };
    }

    #[cfg(not(target_arch = "aarch64"))]
    {
        #[cfg(target_arch = "x86_64")]
        eprintln!("[SCALAR FALLBACK] compute_normals_vector_par: AVX2 not detected");
        #[cfg(not(target_arch = "x86_64"))]
        eprintln!("[SCALAR FALLBACK] compute_normals_vector_par: no SIMD for this architecture");
        return row_major::compute_normals_scalar(hm);
    }
}

pub fn compute_normals_vector_tiled(tiled_hm: &dem_io::TiledHeightmap) -> NormalMap {
    #[cfg(target_arch = "aarch64")]
    return unsafe { tiled::compute_normals_neon_tiled(tiled_hm) };

    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx2") {
        return unsafe { tiled_avx2::compute_normals_avx2_tiled(tiled_hm) };
    }

    #[cfg(not(target_arch = "aarch64"))]
    {
        #[cfg(target_arch = "x86_64")]
        eprintln!(
            "[SCALAR FALLBACK] compute_normals_vector_tiled: AVX2 not detected — using scalar via get()"
        );
        #[cfg(not(target_arch = "x86_64"))]
        eprintln!(
            "[SCALAR FALLBACK] compute_normals_vector_tiled: no SIMD for this architecture — using scalar via get()"
        );
        return tiled::compute_normals_scalar_tiled(tiled_hm);
    }
}

pub fn compute_normals_vector_tiled_par(tiled_hm: &dem_io::TiledHeightmap) -> NormalMap {
    #[cfg(target_arch = "aarch64")]
    return unsafe { tiled::compute_normals_neon_tiled_parallel(tiled_hm) };

    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx2") {
        return unsafe { tiled_avx2::compute_normals_avx2_tiled_parallel(tiled_hm) };
    }

    #[cfg(not(target_arch = "aarch64"))]
    {
        #[cfg(target_arch = "x86_64")]
        eprintln!(
            "[SCALAR FALLBACK] compute_normals_vector_tiled_par: AVX2 not detected — using scalar via get()"
        );
        #[cfg(not(target_arch = "x86_64"))]
        eprintln!(
            "[SCALAR FALLBACK] compute_normals_vector_tiled_par: no SIMD for this architecture — using scalar via get()"
        );
        return tiled::compute_normals_scalar_tiled(tiled_hm);
    }
}

pub fn compute_shadow_vector(hm: &dem_io::Heightmap, sun_elevation_rad: f32) -> ShadowMask {
    #[cfg(target_arch = "aarch64")]
    return unsafe { shadow::compute_shadow_neon(hm, sun_elevation_rad) };

    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx2") {
        return unsafe { shadow_avx2::compute_shadow_avx2(hm, sun_elevation_rad) };
    }

    #[cfg(not(target_arch = "aarch64"))]
    {
        #[cfg(target_arch = "x86_64")]
        eprintln!("[SCALAR FALLBACK] compute_shadow_vector: AVX2 not detected");
        #[cfg(not(target_arch = "x86_64"))]
        eprintln!("[SCALAR FALLBACK] compute_shadow_vector: no SIMD for this architecture");
        return shadow::compute_shadow_scalar(hm, sun_elevation_rad);
    }
}

pub fn compute_shadow_vector_par(hm: &dem_io::Heightmap, sun_elevation_rad: f32) -> ShadowMask {
    #[cfg(target_arch = "aarch64")]
    return unsafe { shadow::compute_shadow_neon_parallel(hm, sun_elevation_rad) };

    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx2") {
        return unsafe { shadow_avx2::compute_shadow_avx2_parallel(hm, sun_elevation_rad) };
    }

    #[cfg(not(target_arch = "aarch64"))]
    {
        #[cfg(target_arch = "x86_64")]
        eprintln!("[SCALAR FALLBACK] compute_shadow_vector_par: AVX2 not detected");
        #[cfg(not(target_arch = "x86_64"))]
        eprintln!("[SCALAR FALLBACK] compute_shadow_vector_par: no SIMD for this architecture");
        return shadow::compute_shadow_scalar(hm, sun_elevation_rad);
    }
}

pub fn compute_shadow_vector_par_with_azimuth(
    hm: &dem_io::Heightmap,
    sun_azimuth_rad: f32,
    sun_elevation_rad: f32,
    penumbra_meters: f32,
) -> ShadowMask {
    #[cfg(target_arch = "aarch64")]
    return unsafe {
        shadow::compute_shadow_neon_parallel_with_azimuth(
            hm,
            sun_azimuth_rad,
            sun_elevation_rad,
            penumbra_meters,
        )
    };

    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx2") {
        return unsafe {
            shadow_avx2::compute_shadow_avx2_parallel_with_azimuth(
                hm,
                sun_azimuth_rad,
                sun_elevation_rad,
                penumbra_meters,
            )
        };
    }

    #[cfg(not(target_arch = "aarch64"))]
    {
        #[cfg(target_arch = "x86_64")]
        eprintln!("[SCALAR FALLBACK] compute_shadow_vector_par_with_azimuth: AVX2 not detected");
        #[cfg(not(target_arch = "x86_64"))]
        eprintln!(
            "[SCALAR FALLBACK] compute_shadow_vector_par_with_azimuth: no SIMD for this architecture"
        );
        return shadow::compute_shadow_scalar_with_azimuth(
            hm,
            sun_azimuth_rad,
            sun_elevation_rad,
            penumbra_meters,
        );
    }
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

pub fn compute_ao_true_hemi(
    hm: &Heightmap,
    n_directions: usize,
    ray_elevation_rad: f32,
    penumbra_meters: f32,
) -> Vec<f32> {
    let mut output: Vec<f32> = vec![0.0f32; hm.rows * hm.cols];

    for i in 0..n_directions {
        let azimuth: f32 = i as f32 * std::f32::consts::TAU / n_directions as f32;
        let mask: ShadowMask =
            compute_shadow_vector_par_with_azimuth(hm, azimuth, ray_elevation_rad, penumbra_meters);

        for j in 0..output.len() {
            output[j] += mask.data[j];
        }
    }

    for x in output.iter_mut() {
        *x /= n_directions as f32;
    }

    output
}
