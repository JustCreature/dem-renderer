//! Tests for the shadow (DDA sweep) and ambient-occlusion kernels.
//!
//! Same two layers as the normals tests: analytic ground truth against the scalar
//! reference, then host-arch SIMD/parallel variants differentially against scalar.
//! AO is the 16-azimuth average of the azimuth shadow sweep, so it gets sanity
//! bounds + a directional-occlusion check rather than an exact oracle.

mod common;

use common::*;
use terrain::*;

// ── analytic: west-only sweep ────────────────────────────────────────────────

#[test]
fn flat_terrain_fully_lit() {
    let h = flat(16, 16, 50.0, 10.0, 10.0);
    let m = compute_shadow_scalar(&h, 0.5);
    assert!(
        m.data.iter().all(|&v| v == 1.0),
        "flat terrain casts no shadow"
    );
}

#[test]
fn single_ridge_casts_known_shadow() {
    // Ground at 0 with a ridge of height 100 at column 5; dx = 10 m, tan(elev) = 0.5.
    // West-only sweep marches east; a flat cell c east of the ridge is shadowed
    // while its h_eff = c·dx·tan stays below the ridge's running max
    // (100 + 5·dx·tan = 125), i.e. while c < 25. So cells 6..=24 are dark.
    let elev = 0.5f32.atan();
    let cols = 40;
    let cx = 5;
    let h = single_ridge(8, cols, cx, 100.0, 10.0, 10.0);
    let m = compute_shadow_scalar(&h, elev);
    let row = 3;

    assert_eq!(m.data[row * cols + 2], 1.0, "west of ridge: lit");
    assert_eq!(
        m.data[row * cols + cx],
        1.0,
        "ridge column itself: lit (occluder)"
    );
    assert_eq!(m.data[row * cols + 10], 0.0, "inside cast shadow: dark");
    assert_eq!(m.data[row * cols + 35], 1.0, "beyond cast shadow: lit");
}

#[test]
fn scalar_matches_branchless() {
    // Identical integer math (one writes 0.0 conditionally, the other writes
    // 1.0 - in_shadow) — must be bit-for-bit equal.
    let h = pseudo_random(40, 40, 11, 500.0, 10.0, 10.0);
    let a = compute_shadow_scalar(&h, 0.4);
    let b = compute_shadow_scalar_branchless(&h, 0.4);
    assert_eq!(a.data, b.data);
}

// ── differential ────────────────────────────────────────────────────────────

#[cfg(target_arch = "aarch64")]
#[test]
fn neon_shadow_matches_scalar_westonly() {
    let h = pseudo_random(70, 55, 0xBEEF, 1000.0, 10.0, 10.0);
    let s = compute_shadow_scalar(&h, 0.6);
    let n = unsafe { compute_shadow_neon(&h, 0.6) };
    let np = unsafe { compute_shadow_neon_parallel(&h, 0.6) };

    // float-order ties may flip a few cells; allow <= 0.5% (the 4-row packing
    // class of bug would flip a large fraction).
    let limit = (s.data.len() / 200).max(2);
    assert!(
        mask_hard_mismatches(&s.data, &n.data) <= limit,
        "neon west-only diverges from scalar"
    );
    assert!(
        mask_hard_mismatches(&s.data, &np.data) <= limit,
        "neon parallel west-only diverges from scalar"
    );
}

#[cfg(target_arch = "aarch64")]
#[test]
fn neon_shadow_matches_scalar_azimuth() {
    // az = 0 gives pure south-going (vertical) rays — one disjoint ray per column,
    // so there is no diagonal-corner overlap and scalar vs NEON agree to ULPs.
    let h = pseudo_random(60, 60, 0xF00D, 1000.0, 10.0, 10.0);
    let (az, elev, pen) = (0.0f32, 0.5f32, 50.0f32);
    let s = compute_shadow_scalar_with_azimuth(&h, az, elev, pen);
    let n = unsafe { compute_shadow_neon_parallel_with_azimuth(&h, az, elev, pen) };
    assert_close_slice(&s.data, &n.data, 1e-3, "neon azimuth");
}

#[cfg(target_arch = "x86_64")]
#[test]
fn avx2_shadow_matches_scalar() {
    if !is_x86_feature_detected!("avx2") {
        eprintln!("avx2 not available on this host — skipping");
        return;
    }
    let h = pseudo_random(70, 55, 0xBEEF, 1000.0, 10.0, 10.0);
    let s = compute_shadow_scalar(&h, 0.6);
    let limit = (s.data.len() / 200).max(2);
    let n = unsafe { compute_shadow_avx2(&h, 0.6) };
    let np = unsafe { compute_shadow_avx2_parallel(&h, 0.6) };
    assert!(
        mask_hard_mismatches(&s.data, &n.data) <= limit,
        "avx2 west-only"
    );
    assert!(
        mask_hard_mismatches(&s.data, &np.data) <= limit,
        "avx2 parallel"
    );

    let (az, elev, pen) = (0.0f32, 0.5f32, 50.0f32);
    let sa = compute_shadow_scalar_with_azimuth(&h, az, elev, pen);
    let na = unsafe { compute_shadow_avx2_parallel_with_azimuth(&h, az, elev, pen) };
    assert_close_slice(&sa.data, &na.data, 1e-3, "avx2 azimuth");
}

// ── ambient occlusion ────────────────────────────────────────────────────────

#[test]
fn ao_flat_is_unoccluded() {
    let h = flat(20, 20, 100.0, 10.0, 10.0);
    let ao = compute_ao_true_hemi(&h, 16, 0.3, 50.0);
    assert_eq!(ao.len(), 20 * 20);
    assert!(
        ao.iter().all(|&v| (v - 1.0).abs() < 1e-3),
        "flat terrain is fully open to the sky"
    );
}

#[test]
fn ao_peak_occludes_downwind_neighbour() {
    // A single peak (height 20, dx = 10 m, tan(elev) ≈ 0.2) casts a shadow ~10
    // cells long in each azimuth. The cell immediately east of the peak loses sky
    // in the east-facing azimuth(s); a cell 18 cells away (beyond the cast length)
    // stays fully open.
    let cols = 40;
    let pr = 20;
    let pc = 20;
    let h = single_peak(40, cols, pr, pc, 20.0, 10.0, 10.0);
    let ao = compute_ao_true_hemi(&h, 16, 0.2, 36.0);

    assert_eq!(ao.len(), 40 * cols);
    assert!(
        ao.iter().all(|&v| (0.0..=1.0001).contains(&v)),
        "AO out of [0,1]"
    );

    let near = ao[pr * cols + pc + 1]; // adjacent, inside cast shadow
    let far = ao[pr * cols + pc + 18]; // beyond cast shadow
    assert!(near < 1.0, "neighbour should be partly occluded: {near}");
    assert!(far > 0.999, "far cell should be open: {far}");
    assert!(
        near < far,
        "near {near} should be more occluded than far {far}"
    );
}

// ── dispatcher edge guards ──────────────────────────────────────────────────

#[test]
fn tiny_inputs_return_neutral_shadow() {
    for (r, c) in [(0, 0), (1, 1), (2, 2), (2, 5), (5, 2), (1, 10)] {
        let h = flat(r, c, 123.0, 5.0, 5.0);
        let m = compute_shadow_vector(&h, 0.5);
        assert_eq!((m.rows, m.cols), (r, c));
        assert_eq!(m.data.len(), r * c);
        assert!(
            m.data.iter().all(|&v| v == 1.0),
            "neutral shadow is all-lit"
        );

        let ma = compute_shadow_vector_par_with_azimuth(&h, 0.3, 0.5, 50.0);
        assert!(ma.data.iter().all(|&v| v == 1.0));
    }
}
