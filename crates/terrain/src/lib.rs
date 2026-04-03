mod row_major;
mod shadow;
mod tiled;

pub use row_major::{
    compute_normals_neon, compute_normals_neon_8, compute_normals_neon_parallel,
    compute_normals_scalar,
};
pub use shadow::{
    ShadowMask, compute_shadow_neon, compute_shadow_neon_parallel, compute_shadow_scalar,
    compute_shadow_scalar_branchless, compute_shadow_scalar_with_azimuth,
};
pub use tiled::{compute_normals_neon_tiled, compute_normals_neon_tiled_parallel};

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
