# Phase 3 Session Log
**Date**: 2026-04-03
**Hardware**: Apple M4 Max
**Branch**: phase_3

---

## Session Goals

Implement sun shadow computation for the heightmap:
- Scalar baseline with horizon-angle sweep
- Branchless vs branchy comparison
- NEON SIMD vectorised across rows (not columns)
- Rayon parallelism
- Generalise to arbitrary sun azimuth via DDA
- Benchmark all variants, understand bottlenecks

---

## Pre-Implementation Design Discussion

**Q: Cardinal vs arbitrary azimuth — does it change the algorithm?**
Yes fundamentally. Cardinal direction sweeps along rows (pure east-west march). Arbitrary azimuth requires diagonal traversal (DDA), different entry edges, different distance accumulation. Designed as two separate implementations: cardinal NEON-parallel first, then DDA generalisation.

**Shadow output format**: `f32` (soft shadow factor) rather than `u8`, to allow partial shadow blending in Phase 4.

**ShadowMask struct decided**: `Vec<f32>`, 1.0=lit, 0.0=shadow, 52 MB for 3601².

---

## Step 1 — Data Structures and DDA Setup

Added `ShadowMask` struct and `DdaSetup` helper to `crates/terrain/src/shadow.rs`.

`DdaSetup` contains: `dc_step`, `dr_step`, `dist_per_step`, `starting_pixels: Vec<(f32,f32)>`.

`dda_setup()` computes: sun direction in grid coords (`dc = -sin(az)`, `dr = cos(az)`), DDA normalisation (dominant-axis = ±1), real-world `dist_per_step` from `dx_meters`/`dy_meters`, and entry-edge starting pixels.

Confirmed `Heightmap` already carries `dx_meters: f64` and `dy_meters: f64` from Phase 1.

---

## Step 2 — Scalar Implementation and h_eff Bug

Initial h_eff formula was wrong: `height - dist * tan_sun`. PNG came out almost all black.

**Root cause**: wrong sign. Flat-terrain test: `h_eff[c] = H - c×dx×tan(θ)` decreases → running max always exceeds current pixel → every pixel in shadow.

**Fix via backward-ray derivation**: trace backward ray from pixel `c` toward sun (westward+upward). At column `c'`, ray height = `height[c] + (c - c') × dx × tan(θ)`. Shadowed if `height[c'] > ray_height(c')`. Rearranging: `h_eff[c] = height[c] + c × dx × tan(θ)` — plus sign.

Other bugs fixed:
- `hm.data[r * hm.cols + r]` (used `r` for `c` in column index) — silent wrong output
- Wrong bandwidth formula: copied normals formula (20 bytes/pixel) instead of shadow's 6 bytes/pixel

**Measured scalar result**: 8.1 GB/s, 10.2ms.

**Bottleneck analysis**:
- Latency chain: `3601² × 2 cycles / 4 GHz ≈ 6.5ms` (64% of runtime)
- Memory: 78 MB at ~6.7 GB/s ≈ 3.7ms (36%)
- Loop is 64% latency-bound — the `running_max.max()` serial dependency chain dominates

---

## Step 3 — Branchless Comparison

Branchless version: `let in_shadow = (h_eff < running_max) as u32 as f32; data[...] = 1.0 - in_shadow;`

**Result**: 10.6 GB/s (31% faster than branchy 8.1 GB/s).

Shadow regions are spatially coherent → TAGE predictor achieves >99% accuracy → few mispredictions. Branchless wins because:
1. Unconditional sequential stores are more pipeline-friendly than conditional irregular ones
2. Branch uops consume ROB slots and frontend bandwidth even when correctly predicted

Lesson: "avoid branches because of misprediction" is an oversimplification — branchless can win even with accurate prediction.

---

## Step 4 — NEON Vectorisation (4 rows simultaneously)

**Key insight**: no intra-row parallelism (each pixel depends on previous `running_max`). Parallelism is between rows — process 4 independent rows at once.

Bugs caught during implementation:
- `vbslq_f32(mask, 1.0, 0.0)` → args swapped. `vcltq_f32` sets mask=1 for shadow → must be `vbslq_f32(mask, 0.0, 1.0)`
- Tried `vst1q_f32` for scatter stores (4 outputs ~14 KB apart) → must use 4 × `vgetq_lane_f32:<N>` + scalar stores
- `while r + 4 < hm.rows` (off-by-one) → `r + 4 <= hm.rows`
- Scalar tail: `r < hm.rows - 1` (last row skipped) → `r < hm.rows`

**Gather pattern**: heights at column `c` across 4 rows are 7.2 KB apart → scalar load into `[f32; 4]` array, then `vld1q_f32`. M4 prefetcher tracks 4 sequential streams without issue.

**Result**: 17.4 GB/s, 4.7ms (2.2× over scalar).

Why 2.2× not 4×: latency component reduced 4× (6.5 → 1.6ms) but memory component also present (3.7ms). Combined: 10.2ms → 4.8ms ≈ 2.1×. Classic bottleneck-shift.

---

## Step 5 — Rayon Parallel

`par_chunks_mut(4 * hm.cols)` over output data — rayon guarantees non-overlapping chunks at compile time. `&Heightmap` is `Sync` (read-only), no locks needed.

Bugs fixed:
- `chunk[r * hm.cols + c]` (global offset) → `chunk[local_r * hm.cols + c]` (within chunk)
- Various off-by-one issues in chunk-local row computation

**Result**: 58.6 GB/s, 1.4ms (7.3× over scalar, 3.4× over single-thread NEON).

Why 3.4× from parallelism not 10×: latency contribution negligible (0.16ms for 10 cores). Memory bandwidth is now the ceiling — 10 threads × 4 streams = 40 concurrent strided streams saturate the unified memory controller.

**Optimization arc**:
```
Scalar:        latency-bound (64%) + memory (36%)  = 10.2ms
NEON 4-wide:   latency reduced 4×, memory revealed  =  4.7ms
NEON parallel: latency negligible, memory ceiling   =  1.4ms
```

---

## Step 6 — DDA for Arbitrary Sun Azimuth

**Algorithm**:
- `dc = -sin(az)`, `dr = cos(az)` (grid coords: col=east, row=south)
- Normalise: dominant axis gets ±1, other axis gets fractional
- Entry edges: west if `dc_step > 0`, east if `< 0`, north if `dr_step > 0`, south if `< 0`
- For diagonal sun: both row-edge and column-edge apply → ~7200 starting pixels
- `dist_per_step = sqrt((dc_step × dx)² + (dr_step × dy)²)` — real metres
- Use `round()` not `floor()` to keep path centred on theoretical line

Good configs for testing at 47°N:
- Equinox sunrise: azimuth=90°, elevation=10° — long west shadows
- Winter solstice noon: azimuth=180°, elevation=19.5° — north shadows

Bugs fixed during DDA implementation:
- `starting_pixels = vec![(0.0, 0.0)]` → `Vec::new()`
- Nested `for c` inside `for r` when collecting starting pixels → `for r` only (pixels, not pixel×column pairs)
- Missing dr_step cases for north/south entry edges
- `dda_setup` called inside the ray loop (recomputed per pixel) → moved outside

---

## Step 7 — `compute_shadow_neon_parallel_with_azimuth`

Combined NEON + rayon + arbitrary azimuth. Uses `par_chunks(4)` on `starting_pixels` (not `par_chunks_mut` on data) because rays write to scattered positions → `SendPtr` needed.

**Benign race condition**: diagonal rays from two entry edges can visit the same corner pixel. On AArch64, 32-bit store is a single atomic instruction → no torn writes. Both writers produce valid `f32` (0.0 or 1.0). Acceptable for visualisation.

Two-phase NEON approach:
1. NEON loop while ALL 4 rays are in bounds (full vectorisation)
2. `vgetq_lane_f32:<N>` to extract per-lane `running_max`, scalar continuation per surviving ray

`dda_setup()` helper extracted and shared between scalar and NEON-parallel implementations.

---

## Artifacts Produced

- `crates/terrain/src/shadow.rs` — complete: `DdaSetup`, `dda_setup`, `ShadowMask`, `compute_shadow_scalar`, `compute_shadow_scalar_branchless`, `compute_shadow_scalar_with_azimuth`, `compute_shadow_neon`, `compute_shadow_neon_parallel`, `compute_shadow_neon_parallel_with_azimuth`
- `crates/terrain/src/lib.rs` — exports updated
- `src/main.rs` — benchmark functions for all shadow variants, `create_rgb_png` added
- `docs/lessons/phase-3/long-report.md` — comprehensive student textbook
- `docs/lessons/phase-3/short-report.md` — reference document

---

## Performance Summary (M4 Max, cold cache, isolated runs)

| Implementation | Time | Bandwidth |
|---|---|---|
| Scalar (branchy) | 10.2ms | 8.1 GB/s |
| Scalar (branchless) | 9.4ms | 10.6 GB/s |
| NEON 4-wide | 4.7ms | 17.4 GB/s |
| NEON parallel (10 cores) | 1.4ms | 58.6 GB/s |

`compute_shadow_neon_parallel_with_azimuth` compiled but not yet benchmarked — pending confirmation of compilation and test run at various azimuths.

---

## Open Items for Phase 4

- Benchmark `compute_shadow_scalar_with_azimuth` and `compute_shadow_neon_parallel_with_azimuth` at various azimuths; compare diagonal vs cardinal performance
- `profiling::timed` label bug: `random_read`/`seq_write`/`random_write` report label `"seq_read"`
- `fill_nodata` division-by-zero if all 4 directions hit boundary
- Tiled normal computation cross-tile boundary rows = zero (halo exchange needed)
