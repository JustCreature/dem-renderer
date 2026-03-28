# Phase 2 Session Log
**Date**: 2026-03-28
**Hardware**: Apple M4 Max
**Branch**: phase_2

---

## Session Goals

Implement surface normal computation for the heightmap:
- Scalar baseline with SoA output layout
- Explicit NEON SIMD with rsqrt + Newton-Raphson
- Rayon parallelism across rows
- Benchmark all three, understand bottlenecks

---

## Pre-Implementation Design Discussion

Worked through the memory arithmetic before writing code:

**Q1 — Store bottleneck?**
192 MB of SoA output at 20 GB/s ≈ 9.6ms. Compute (rsqrt path) also ~6–10ms. Both in the same order — neither dominates obviously. Measurement needed.

**Q2 — Tile fits in L1?**
128×128 × 2 bytes = 32 KB. M4 L1D = 128 KB. Comfortable, including halo.

**Q3 — Write pattern trains prefetcher?**
Within a tile row: stride 4 bytes — yes. Between tile rows: 14 KB jump. Only 3 simultaneous active streams (one per SoA array) — within hardware tracker capacity. Fine.

**AoS vs SoA discussion**: SoA chosen. Loading 4 consecutive `nx` values — one 16-byte load vs AoS's 4 struct loads + shuffles. 16× more useful data per cache line on single-field passes.

---

## Step 1 — NormalMap Struct and Scalar Function

Added to `crates/terrain/src/lib.rs`:

```rust
pub struct NormalMap {
    pub nx: Vec<f32>,
    pub ny: Vec<f32>,
    pub nz: Vec<f32>,
    pub rows: usize,
    pub cols: usize,
}
```

Errors encountered and fixed:
- `crates/terrain/Cargo.toml` had `path = "crates/dem_io"` — wrong. Fixed to `path = "../dem_io"` (relative to crate directory, not workspace root)
- Vecs initialized with `vec![1.0f32]` (wrong size) → fixed to `vec![0.0f32; hm.rows * hm.cols]`
- `std::intrinsics::sqrtf32` import — nightly internal, removed. Used `f32::sqrt()` instead
- Loop bounds `1..hm.rows` → `1..hm.rows - 1` (avoid out-of-bounds on `r+1`)
- `single_nx * single_nx * single_ny * single_ny` (multiplication) → `+` (addition) in length formula
- Results computed but never stored — added `nx[r * hm.cols + c] = ...` writes
- `2 * hm.dx_meters` integer literal ambiguity → `2.0 * hm.dx_meters`
- `mut` on `single_nx`, `single_ny`, `single_nz` — unnecessary, removed

Final scalar implementation: finite differences with real `dx_meters`/`dy_meters`, `nz = 1.0`, `f32::sqrt` normalization, writes to SoA arrays.

---

## Step 2 — Visual Check (PNG)

Added `nz` → grayscale PNG output. Flat terrain = white (nz ≈ 1.0), steep slopes = darker. Output looked correct — recognizable terrain with realistic shading. Also added a 500×500 crop (`r_start=1000, c_start=1500`) for zooming into mountain detail.

Fixed borrow error in crop code: closure captured `normal_map.nz` by move inside `flat_map` — fixed by binding `let nz = &normal_map.nz` outside the closure.

---

## Step 3 — Scalar Benchmark Result

```
compute_normals_scalar_from_row_major_hm: 24.3 GB/s  (isolated cold)
```

Bandwidth formula: `4 * 2 * rows * cols` (reads) + `3 * 4 * rows * cols` (writes) = ~260 MB.

**Auto-vectorization discovered**: assembly check showed `fsqrt.4s v17, v17` and `fsqrt.4s v16, v16` — LLVM had vectorized the loop to 8 pixels per iteration (two 4-lane registers). True scalar (with `std::hint::black_box` barrier): **8.1 GB/s**. Auto-vectorized: **24.3 GB/s** (3× difference).

---

## Step 4 — Explicit NEON Implementation

Added `compute_normals_neon` in `crates/terrain/src/lib.rs`.

**Structure**:
- `unsafe fn` — required for `core::arch::aarch64` intrinsics
- Inner loop: `while c + 4 < hm.cols - 1` (4-wide NEON), scalar tail
- Load chain: `vld1_s16` → `vmovl_s16` → `vcvtq_f32_s32` for each neighbor
- Math: `vmulq_f32(vsubq_f32(left, right), inv_2dx)` for `vec_nx`
- rsqrt + Newton-Raphson normalization
- Store: `vst1q_f32(nx_ptr.add(offset), result)`

**Bugs fixed during implementation**:
- Copy-paste: all four loads used `ptr_upper` — fixed to use respective `ptr_lower`, `ptr_left`, `ptr_right`
- `vec_nz = vld1_f32([1.0, 1.0, 1.0, 1.0].as_ptr())` → `vdupq_n_f32(1.0)` (one instruction, no temp array)
- Redundant inner `unsafe {}` block inside `unsafe fn` — removed
- `vst1q_f32` with `nx[r * cols + c]` (scalar index) → `nx.as_mut_ptr().add(...)` (pointer)
- `nx_p.add(...) = value` syntax error → `nx_p.add(...).write(value)`

**Critical performance bug — division in hot loop**:
Initial: `vdivq_f32(..., vdupq_n_f32(2.0 * dx))` inside inner loop.
`2.0 * dx` is loop-invariant. `vdivq_f32` throughput: 10–15 cycles. `vmulq_f32` throughput: 1 cycle.

Fix: hoist reciprocals outside both loops:
```rust
let inv_2dx = vdupq_n_f32(1.0 / (2.0 * hm.dx_meters as f32));
let inv_2dy = vdupq_n_f32(1.0 / (2.0 * hm.dy_meters as f32));
```

**Impact**: 18.3 → 28.8 GB/s (cold isolated).

**Bug discovered**: function was never actually called — `main.rs` was calling `compute_normals_scalar` in both benchmark functions. Fixed; required adding `unsafe { compute_normals_neon(...) }` at call site.

**8-wide unroll**: tried `c += 8` with two independent groups (A and B). Bug: group B stores used `r * cols + c` instead of `r * cols + c + 4`. Fixed. Result: **27.5 GB/s** — slightly *slower* than 4-wide (register pressure, ~24 live SIMD registers, approaching 32-register limit).

---

## Step 5 — Cache Warmup Analysis

Running benchmarks in sequence without eviction produced misleading numbers. Swapping order showed the effect:

| Order | Run 1 | Run 2 |
|---|---|---|
| Scalar then NEON | Scalar cold 22.6 | NEON warm 38.3 |
| NEON then scalar | NEON cold 18.3→26.9 | Scalar warm 28.7 |
| Isolated each | Scalar 24.3 | NEON 28.8 |

Eviction buffer needed: ~320 MB (26 MB reads + 147 MB outputs × 2). The output arrays are freshly allocated each run but the allocator may return the same pages — warm in cache from the previous run's writes.

**Clean isolated cold numbers**:
- Scalar: 24.3 GB/s
- NEON 4-wide: 28.8 GB/s (+19%)
- NEON 8-wide: 27.5 GB/s (-1.3 GB/s from register pressure)

---

## Step 6 — Rayon Parallelism

Added `rayon = "1"` to `crates/terrain/Cargo.toml`.

**Ownership problem**: multiple threads writing to `nx`, `ny`, `nz` — compiler rejects simultaneous `&mut Vec<f32>`.

**Solution — SendPtr pattern**:
```rust
struct SendPtr(*mut f32);
unsafe impl Send for SendPtr {}
unsafe impl Sync for SendPtr {}
```

**Errors encountered**:

1. `*mut f32 cannot be shared between threads safely (Sync not implemented)` — first occurrence: `float32x4_t` captured by reference across thread boundary. `float32x4_t` contains `*mut f32` internally → `!Sync`. Fix: capture `f32` scalars, create NEON constants inside closure.

2. Same error persists — non-move closure still captured something `!Sync`. Fix: add `move` to closure so `SendPtr` values are captured by value (not reference). `SendPtr: Sync` ✓.

3. NEON intrinsics called in safe closure — `unsafe {}` does not propagate into closure bodies. Fix: add `unsafe {}` block inside the closure body.

4. NEON stores still using `nx.as_mut_ptr()` inside the parallel closure (capturing `&mut Vec<f32>` → `!Sync`). Fixed to use `nx_p`/`ny_p`/`nz_p` from `SendPtr`.

**Final parallel implementation**: `(1..rows-1).into_par_iter().for_each(move |r| { unsafe { ... } })`.

---

## Final Results

| Implementation | Cold GB/s | Notes |
|---|---|---|
| True scalar | 8.1 | black_box barrier |
| Auto-vectorized scalar | 24.3 | LLVM emits fsqrt.4s |
| NEON 4-wide | 28.8 | explicit intrinsics |
| NEON 8-wide | 27.5 | register pressure |
| NEON parallel 10 cores | 50.5 | rayon, cold |
| NEON parallel warm | ~117 | warm cache |

**Parallel cold scaling**: 50.5 / 28.8 = 1.75× from 10 cores → DRAM write-bandwidth bound. 147 MB output exceeds 16 MB L3. All cores share the same memory bus.

**Parallel warm scaling**: 117 / ~47 ≈ 2.5× → L3 bandwidth is higher and scales better.

---

---

## Step 7 — Tiled Normal Computation

Implemented `compute_normals_neon_tiled` and `compute_normals_neon_tiled_parallel` in `crates/terrain/src/tiled.rs`.

**Motivation**: row-major normal computation reads upper/lower neighbors with stride `cols * 2 = 7202` bytes. Tiled layout gives stride `tile_size * 2 = 256` bytes — should be more prefetcher-friendly.

**Key implementation differences from row-major**:
- Outer loops: `for tr in 0..tile_rows { for tc in 0..tile_cols { ... } }`
- Tile pointer: `let tile_ptr = hm.tiles().as_ptr().add(tr * tile_cols + tc) * tile_size²)`
- Neighbor stride: `tile_ptr.add((local_r ± 1) * tile_size + local_c)` — stride `tile_size` bytes
- Cross-tile boundary: skip `local_r = 0` and `local_r = tile_size - 1` (neighbors in adjacent tile not accessible)
- Output: `global_r * hm.cols + global_c` where `global_r = tr * tile_size + local_r`
- Parallel version: `(0..tile_rows * tile_cols).into_par_iter()`, decomposes via `tr = tile_idx / tile_cols`

**Bug — Rust 2021 precise disjoint capture**:
```rust
// BROKEN: captures nx_ptr.0 (*mut f32) — bypasses SendPtr's Send/Sync impls
vst1q_f32(nx_ptr.0.add(out), ...);

// FIXED: captures nx_ptr (SendPtr) — Send+Sync holds
vst1q_f32(nx_ptr.get().add(out), ...);
```
Rust 2021 edition changed closure capture to minimal-path capture. `nx_ptr.0` is a field path of type `*mut f32`, so that's what the closure captures — not `nx_ptr: SendPtr`. The `unsafe impl Sync for SendPtr {}` is bypassed. Solution: add a `get()` method on `SendPtr` and call it instead of accessing `.0` directly.

**Results (isolated cold)**:

| Implementation | GB/s |
|---|---|
| Row-major NEON single-thread | 24.1 |
| Tiled NEON single-thread | 22.9 |
| Row-major NEON parallel | 42.3 |
| Tiled NEON parallel | 34.0 |

**Analysis**:

Tiled is slower in both cases.

Single-thread (~5% worse): The M4 prefetcher already handles the 7202-byte stride in row-major (as seen in Phase 1 neighbor-sum: 39 GB/s). Tiled adds overhead — global coordinate computation, extra boundary guards, skipped border rows — that nearly cancels the input locality gain.

Parallel (19% worse): The root cause is the output write pattern. Row-major parallel: each thread writes a contiguous range of rows → perfectly sequential streaming stores. Tiled parallel: tile (tr, tc) writes to rows `[tr*128..(tr+1)*128]` × cols `[tc*128..(tc+1)*128]` in a row-major output. Consecutive rows within a tile are `cols * 4 = 14404` bytes apart. Multiple threads writing to interleaved column ranges creates more scattered write-allocate traffic on the shared memory bus.

**The fundamental asymmetry**: tiled layout improves *input* read locality (neighbor stride: 256 bytes vs 7202 bytes), but the *output* is still row-major SoA. The output is ~6× larger than the input (156 MB vs 26 MB), so writes dominate bandwidth — and tiled makes them worse.

**When tiled would win**: if the output were also stored in tiled layout, or for a diagonal sweep (Phase 3 shadow with non-horizontal sun) where row-major accesses are cache-thrashing.

---

## Concepts Covered

- Surface normals via finite differences
- Lambertian shading and why normalization is required
- AoS vs SoA — cache line utilization, SIMD implications
- NEON intrinsic chain: `vld1_s16` → `vmovl_s16` → `vcvtq_f32_s32`
- SIMD stencil window trick — left/right offsets, lane alignment
- `vrsqrteq_f32` + `vrsqrtsq_f32` Newton-Raphson reciprocal sqrt
- Division vs multiplication throughput (why division is hard in hardware)
- Auto-vectorization detection via `--emit=asm`
- Cache warmup as a benchmark artifact — eviction strategy
- Rayon + raw pointer thread safety: `Send`, `Sync`, `SendPtr`
- `unsafe` propagation rules — closures need their own `unsafe {}` blocks
- SIMD types (`float32x4_t`) are `!Sync` — capture scalars across thread boundaries
- Memory-bandwidth ceiling: why parallel scaling stops before core count
- Rust 2021 precise disjoint capture — field access `.0` captures the field, not the struct
- Tiled layout input/output asymmetry — read locality gain erased by scattered output writes
- When tiled layout wins: diagonal access patterns, or tiled output layout
