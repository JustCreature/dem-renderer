//! Tests for the surface-normal kernels.
//!
//! Two layers:
//!  - **Analytic** — `compute_normals_scalar` against hand-derived ground truth
//!    (flat plane, constant slope, untouched borders). This anchors scalar as the
//!    trustworthy oracle.
//!  - **Differential** — every host-arch SIMD/parallel backend and the public
//!    dispatchers vs scalar. On one machine only the host arch's SIMD path runs
//!    (everything is `#[cfg(target_arch)]`-gated), so this transitively validates
//!    that path against the analytically-anchored scalar reference.

mod common;

use common::*;
use terrain::*;

const EPS: f32 = 1e-3;

// ── analytic ──────────────────────────────────────────────────────────────────

#[test]
fn flat_terrain_normals_point_up() {
    let cols = 16;
    let h = flat(16, cols, 100.0, 5.0, 5.0);
    let n = compute_normals_scalar(&h);
    for r in 1..15 {
        for c in 1..15 {
            let i = r * cols + c;
            assert!(n.nx[i].abs() < EPS, "nx[{i}] = {}", n.nx[i]);
            assert!(n.ny[i].abs() < EPS, "ny[{i}] = {}", n.ny[i]);
            assert!((n.nz[i] - 1.0).abs() < EPS, "nz[{i}] = {}", n.nz[i]);
        }
    }
}

#[test]
fn constant_slope_gives_known_normal() {
    let cols = 16;
    let rise = 2.0f32; // height units per column
    let dx = 5.0f64;
    let h = ramp_x(16, cols, rise, dx, 5.0);
    let n = compute_normals_scalar(&h);

    // Sobel central difference: single_nx = (left - right)/(2·dx) = -rise/dx,
    // single_ny = 0, single_nz = 1, then normalise.
    let raw_nx = -rise / dx as f32;
    let len = (raw_nx * raw_nx + 1.0).sqrt();
    let (ex, ey, ez) = (raw_nx / len, 0.0, 1.0 / len);

    for r in 1..15 {
        for c in 1..15 {
            let i = r * cols + c;
            assert!((n.nx[i] - ex).abs() < EPS, "nx[{i}] = {} vs {ex}", n.nx[i]);
            assert!((n.ny[i] - ey).abs() < EPS, "ny[{i}] = {} vs {ey}", n.ny[i]);
            assert!((n.nz[i] - ez).abs() < EPS, "nz[{i}] = {} vs {ez}", n.nz[i]);
            let mag = n.nx[i] * n.nx[i] + n.ny[i] * n.ny[i] + n.nz[i] * n.nz[i];
            assert!((mag - 1.0).abs() < EPS, "unit length: {mag}");
        }
    }
}

#[test]
fn borders_remain_zero() {
    let cols = 8;
    let h = ramp_x(8, cols, 3.0, 5.0, 5.0);
    let n = compute_normals_scalar(&h);
    // The kernel writes only 1..dim-1; the frame must stay exactly zero.
    for c in 0..cols {
        for r in [0usize, 7] {
            let i = r * cols + c;
            assert_eq!((n.nx[i], n.ny[i], n.nz[i]), (0.0, 0.0, 0.0));
        }
    }
    for r in 0..8 {
        for c in [0usize, 7] {
            let i = r * cols + c;
            assert_eq!((n.nx[i], n.ny[i], n.nz[i]), (0.0, 0.0, 0.0));
        }
    }
}

// ── differential ────────────────────────────────────────────────────────────

fn assert_normals_close(a: &NormalMap, b: &NormalMap, eps: f32, ctx: &str) {
    assert_eq!((a.rows, a.cols), (b.rows, b.cols), "{ctx}: dims");
    assert_close_slice(&a.nx, &b.nx, eps, &format!("{ctx} nx"));
    assert_close_slice(&a.ny, &b.ny, eps, &format!("{ctx} ny"));
    assert_close_slice(&a.nz, &b.nz, eps, &format!("{ctx} nz"));
}

#[cfg(target_arch = "aarch64")]
#[test]
fn neon_normals_match_scalar() {
    // Odd dims (67×53) so the SIMD body and the scalar column tail both run.
    let h = pseudo_random(67, 53, 0xDEAD_BEEF, 1000.0, 5.0, 5.0);
    let s = compute_normals_scalar(&h);

    assert_normals_close(&s, &unsafe { compute_normals_neon(&h) }, EPS, "neon");
    assert_normals_close(
        &s,
        &unsafe { compute_normals_neon_parallel(&h) },
        EPS,
        "neon_parallel",
    );
}

#[cfg(target_arch = "x86_64")]
#[test]
fn avx2_normals_match_scalar() {
    if !is_x86_feature_detected!("avx2") {
        eprintln!("avx2 not available on this host — skipping");
        return;
    }
    let h = pseudo_random(67, 53, 0xDEAD_BEEF, 1000.0, 5.0, 5.0);
    let s = compute_normals_scalar(&h);
    assert_normals_close(&s, &unsafe { compute_normals_avx2(&h) }, EPS, "avx2");
    assert_normals_close(
        &s,
        &unsafe { compute_normals_avx2_parallel(&h) },
        EPS,
        "avx2_parallel",
    );
}

#[test]
fn dispatchers_match_scalar() {
    let h = pseudo_random(64, 64, 7, 800.0, 10.0, 10.0);
    let s = compute_normals_scalar(&h);
    assert_normals_close(&s, &compute_normals_vector(&h), EPS, "vector");
    assert_normals_close(&s, &compute_normals_vector_par(&h), EPS, "vector_par");
}

// ── dispatcher edge guards ──────────────────────────────────────────────────

#[test]
fn tiny_inputs_return_neutral_normals() {
    // rows/cols below the 3×3 kernel must yield neutral (0,0,1) output, never a
    // panic or a usize underflow (see lib.rs MIN_KERNEL_DIM guard).
    for (r, c) in [(0, 0), (1, 1), (2, 2), (2, 5), (5, 2), (1, 10)] {
        let h = flat(r, c, 123.0, 5.0, 5.0);
        let n = compute_normals_vector(&h);
        assert_eq!((n.rows, n.cols), (r, c));
        assert_eq!(n.nx.len(), r * c);
        for i in 0..r * c {
            assert_eq!((n.nx[i], n.ny[i], n.nz[i]), (0.0, 0.0, 1.0));
        }
    }
}
