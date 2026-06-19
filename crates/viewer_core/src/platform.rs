//! Platform-bound seams behind small traits. `viewer_core` is otherwise
//! platform-clean; these two traits are the only places the engine touches the
//! outside world. Native implementations live in [`crate::platform_native`]
//! (filesystem + `std::thread`); a wasm shell supplies its own (in-memory bytes
//! + a Web Worker pool).

use std::path::Path;

use dem_io::Heightmap;

/// Reads DEM windows for the streaming tier workers.
///
/// The `key: &Path` is an **opaque tile identifier**, not necessarily a real
/// filesystem path: the native impl treats it as one, a wasm impl maps it to
/// in-memory bytes (and, later, HTTP Range requests). Errors are stringified so
/// the trait stays free of `dem_io`'s native error type.
///
/// Implementations must be `Send + Sync` because the tier workers run on spawned
/// jobs and hold an `Arc<dyn TileSource>`.
pub trait TileSource: Send + Sync {
    /// Extract a window of `radius_m` metres centred on `centre_crs` (the tile's
    /// native CRS coordinates) at overview level `ifd`.
    fn read_window(
        &self,
        key: &Path,
        centre_crs: (f64, f64),
        radius_m: f64,
        ifd: usize,
    ) -> Result<Heightmap, String>;

    /// Read the entire tile (auto-detecting the CRS), used by the geographic
    /// single-tile path and as a last-resort fallback.
    fn read_full(&self, key: &Path) -> Result<Heightmap, String>;

    /// Centre point of the tile in its native CRS, without loading pixel data.
    fn tile_centre_crs(&self, key: &Path) -> Result<(f64, f64), String>;

    /// Pixel scale of each overview (IFD) level, finest first.
    fn ifd_scales(&self, key: &Path) -> Result<Vec<f64>, String>;
}

/// Runs a job off the main thread. Replaces the direct `std::thread::spawn`
/// sites in the original viewer so the shell decides the threading model:
/// `std::thread` natively, a Web Worker pool (`wasm_thread` /
/// `wasm-bindgen-rayon`) on wasm.
///
/// Job closures must carry **only plain data** (`Heightmap`, `NormalMap`,
/// `ShadowMask`, `Vec<u8>`) plus an `Arc<dyn TileSource>` — never a wgpu handle,
/// which is `!Send` on wasm. Results flow back over `std::sync::mpsc`, which is
/// portable.
pub trait Spawner: Send + Sync {
    fn spawn(&self, job: Box<dyn FnOnce() + Send + 'static>);
}
