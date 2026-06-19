//! Constants shared across the viewer core. Mirrors the binary's `src/consts.rs`
//! for the subset the portable viewer logic depends on.

/// Metres per degree of latitude (mean meridional arc length).
/// Longitude degrees scale by an additional cos(lat) factor.
pub(crate) const M_PER_DEG: f64 = 111_320.0;

/// Maximum texture dimension accepted by wgpu without error.
/// Applied before every GPU upload so tiles with no overviews (e.g. 1m NZ LiDAR, 24000px wide)
/// never exceed the hardware texture dimension limit.
pub(crate) const GPU_SAFE_PX: usize = 8192;
