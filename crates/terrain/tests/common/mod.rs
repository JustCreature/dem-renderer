//! Shared fixtures for the terrain integration tests.
//!
//! The terrain kernels only ever read `data`, `rows`, `cols`, `dx_meters` and
//! `dy_meters` from a `Heightmap`; the geo/CRS fields are irrelevant to normals,
//! shadow and AO, so [`hm`] fills them with neutral placeholders. Every builder is
//! deterministic so the differential tests (scalar vs SIMD) are reproducible across
//! runs and machines.

#![allow(dead_code)] // each test binary uses a different subset of these helpers

use dem_io::Heightmap;

/// Build a `Heightmap` from raw row-major data + cell size. The geo/CRS fields are
/// neutral placeholders — the terrain kernels never read them.
pub fn hm(rows: usize, cols: usize, data: Vec<f32>, dx_meters: f64, dy_meters: f64) -> Heightmap {
    assert_eq!(data.len(), rows * cols, "data length must equal rows*cols");
    Heightmap {
        data,
        rows,
        cols,
        nodata: -9999.0,
        origin_lat: 0.0,
        origin_lon: 0.0,
        dx_deg: 0.0,
        dy_deg: 0.0,
        dx_meters,
        dy_meters,
        crs_origin_x: 0.0,
        crs_origin_y: 0.0,
        crs_epsg: 0,
        crs_proj4: String::new(),
    }
}

/// Flat terrain: every cell at height `h`.
pub fn flat(rows: usize, cols: usize, h: f32, dx: f64, dy: f64) -> Heightmap {
    hm(rows, cols, vec![h; rows * cols], dx, dy)
}

/// Constant east-west ramp: height rises by `rise_per_col` for each column index,
/// identical across rows. Produces a constant E-W gradient and zero N-S gradient.
pub fn ramp_x(rows: usize, cols: usize, rise_per_col: f32, dx: f64, dy: f64) -> Heightmap {
    let mut data = vec![0.0f32; rows * cols];
    for r in 0..rows {
        for c in 0..cols {
            data[r * cols + c] = rise_per_col * c as f32;
        }
    }
    hm(rows, cols, data, dx, dy)
}

/// Flat ground at height 0 with a single N-S ridge (one full column) at
/// `ridge_col` raised to `height`.
pub fn single_ridge(
    rows: usize,
    cols: usize,
    ridge_col: usize,
    height: f32,
    dx: f64,
    dy: f64,
) -> Heightmap {
    let mut data = vec![0.0f32; rows * cols];
    for r in 0..rows {
        data[r * cols + ridge_col] = height;
    }
    hm(rows, cols, data, dx, dy)
}

/// Flat ground at height 0 with a single raised cell (a peak) at `(pr, pc)`.
pub fn single_peak(
    rows: usize,
    cols: usize,
    pr: usize,
    pc: usize,
    height: f32,
    dx: f64,
    dy: f64,
) -> Heightmap {
    let mut data = vec![0.0f32; rows * cols];
    data[pr * cols + pc] = height;
    hm(rows, cols, data, dx, dy)
}

/// Deterministic pseudo-random terrain (xorshift64) with values in `[0, amplitude)`.
pub fn pseudo_random(
    rows: usize,
    cols: usize,
    seed: u64,
    amplitude: f32,
    dx: f64,
    dy: f64,
) -> Heightmap {
    let mut state = seed | 1; // avoid the all-zero fixed point
    let mut data = vec![0.0f32; rows * cols];
    for v in data.iter_mut() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let unit = (state >> 11) as f32 / (1u64 << 53) as f32; // [0, 1)
        *v = unit * amplitude;
    }
    hm(rows, cols, data, dx, dy)
}

/// Assert two f32 slices agree within a combined absolute/relative tolerance.
///
/// The NEON/AVX2 normal kernels use a reciprocal-sqrt estimate plus one
/// Newton-Raphson refinement, so they only *approximately* equal the scalar
/// `1.0/sqrt` — never compare these with `==`.
pub fn assert_close_slice(a: &[f32], b: &[f32], eps: f32, ctx: &str) {
    assert_eq!(a.len(), b.len(), "{ctx}: length mismatch");
    for (i, (&x, &y)) in a.iter().zip(b.iter()).enumerate() {
        let diff = (x - y).abs();
        let tol = eps * (1.0 + x.abs().max(y.abs()));
        assert!(
            diff <= tol,
            "{ctx}: index {i}: {x} vs {y} (diff {diff} > tol {tol})"
        );
    }
}

/// Count cells where two shadow masks disagree by more than 0.5 — i.e. a hard
/// `0 ↔ 1` flip.
///
/// West-only scalar and NEON compute `h_eff` with slightly different float
/// orderings (`(c·dx)·tan` vs `c·(dx·tan)`), so a handful of exact ties may flip;
/// the 4-row packing kind of bug this guards against would flip a large fraction
/// of cells instead.
pub fn mask_hard_mismatches(a: &[f32], b: &[f32]) -> usize {
    a.iter()
        .zip(b.iter())
        .filter(|(x, y)| (**x - **y).abs() > 0.5)
        .count()
}
