# Phase 2: Normal Computation — Reference Document

---

## 1. Surface Normals

A normal vector points perpendicularly outward from the surface at each pixel. It encodes orientation, not position. Used for Lambertian shading: `brightness = dot(normal, sun_dir)`. Both vectors must be unit length for `dot` to equal `cos(angle)`. Without normalization, `dot = |normal| × |sun| × cos(angle)` — wrong by slope magnitude.

**Finite differences** (central difference, 1-pixel stencil):
```
nx = (h[r][c-1] - h[r][c+1]) / (2 * dx_meters)
ny = (h[r-1][c] - h[r+1][c]) / (2 * dy_meters)
nz = 1.0
normalize(nx, ny, nz)
```
Loop bounds: `r in 1..rows-1`, `c in 1..cols-1`. Border stays zero.

**Cell sizes** (N47 tile, 47°N): `dy ≈ 30.9 m`, `dx ≈ 21.1 m`. Must use real meters — using 1.0 makes normals geometrically wrong (east-west slopes appear steeper than they are).

---

## 2. AoS vs SoA

**AoS** `Vec<Normal { nx, ny, nz, pad }>` — 16 bytes/pixel. Loading 4 `nx` values requires 4 struct loads + shuffles. 75% of each cache line wasted on unneeded fields during single-field passes.

**SoA** `struct NormalMap { nx: Vec<f32>, ny: Vec<f32>, nz: Vec<f32> }` — 4 bytes/pixel per array. Loading 4 `nx` values: one 16-byte aligned load, zero shuffles, 100% utilization.

**This project uses SoA.** Total output: 3 × 4 × 3601² ≈ 156 MB.

---

## 3. Normalization

```rust
let length = f32::sqrt(nx*nx + ny*ny + nz*nz);
nx /= length;  ny /= length;  nz /= length;
```

In NEON, use `vrsqrteq_f32` + one Newton-Raphson step for `1/sqrt`, then multiply:
```rust
let est = vrsqrteq_f32(len_sq);
let refined = vmulq_f32(vrsqrtsq_f32(vmulq_f32(len_sq, est), est), est);
// refined ≈ 1/sqrt(len_sq) to ~24-bit accuracy
nx_out = vmulq_f32(vec_nx, refined);
```

`vrsqrtsq_f32(a, b)` computes `(2 - a*b) / 2` — the Newton-Raphson correction factor.

---

## 4. NEON Implementation

**Load 4 neighbors for 4 pixels at once** (i16 → f32 conversion pipeline):
```rust
let upper = vcvtq_f32_s32(vmovl_s16(vld1_s16(ptr_upper)));
// vld1_s16: load 4×i16 (64-bit) → int16x4_t
// vmovl_s16: sign-extend → int32x4_t
// vcvtq_f32_s32: convert → float32x4_t
```

**SIMD stencil trick**: for 4 pixels at columns `c..c+4`:
- `left` window starts at `c-1`, `right` window at `c+1`
- `vsubq_f32(left, right)` — lane 0 gets `h[c-1]-h[c+1]`, lane 1 gets `h[c]-h[c+2]`, etc.
- Each lane automatically computes the correct finite difference. No gather needed.

**Store results**:
```rust
vst1q_f32(nx_ptr.add(r * cols + c), vmulq_f32(vec_nx, refined));
```

**Loop structure**: `while c + 4 < cols - 1 { ... c += 4; }` + scalar tail for remainder.

---

## 5. Critical Bug: Division in Hot Loop

```rust
// WRONG — vdivq_f32 throughput: ~10-15 cycles
let vec_nx = vdivq_f32(vsubq_f32(left, right), vdupq_n_f32(2.0 * dx));

// CORRECT — vmulq_f32 throughput: 1 cycle
let inv_2dx = vdupq_n_f32(1.0 / (2.0 * hm.dx_meters as f32));  // hoisted outside loops
let vec_nx = vmulq_f32(vsubq_f32(left, right), inv_2dx);
```

Division is iterative (digit-by-digit); multiplication is a fixed parallel circuit. Division cannot be pipelined across the same operation. Always hoist loop-invariant division as a reciprocal multiply.

**Impact**: 18.3 → 28.8 GB/s after fix.

---

## 6. Auto-Vectorization Discovery

The scalar implementation was auto-vectorized by LLVM. Assembly check:
```sh
cargo rustc -p terrain --release -- --emit=asm && \
  grep -i "fsqrt" target/release/deps/terrain-*.s
```
Output: `fsqrt.4s v17, v17` — NEON 4-lane sqrt. LLVM emitted SIMD from scalar code.

True scalar (with `black_box` barrier): 8.1 GB/s. Auto-vectorized "scalar": 24.3 GB/s. Factor: 3×.

**Lesson**: always check assembly. Auto-vectorization is real but fragile — a branch or dependency can silently disable it.

---

## 7. unsafe in Rayon Closures

**`unsafe {}` does NOT propagate into closure bodies.** A closure defined inside `unsafe {}` is still a safe function — it needs its own `unsafe {}` block:

```rust
unsafe {
    (1..rows-1).into_par_iter().for_each(move |r| {
        unsafe {  // required — outer unsafe does not propagate here
            let v = vld1_s16(ptr);
        }
    });
}
```

**SIMD types (`float32x4_t`) are not `Sync`** — they contain `*mut f32` internally. Do not capture them across thread boundaries. Instead, capture `f32` scalars and call `vdupq_n_f32` inside the closure (1 instruction per thread).

---

## 8. SendPtr Pattern

```rust
struct SendPtr(*mut f32);
unsafe impl Send for SendPtr {}
unsafe impl Sync for SendPtr {}
```

**`Send`**: safe to transfer ownership to another thread.
**`Sync`**: safe to share `&T` across threads.
Raw `*mut f32` is neither by default (compiler assumes aliasing). The newtype asserts: "I manually guarantee no two threads access the same index."

Usage:
```rust
let nx_ptr = SendPtr(nx.as_mut_ptr());
// ... in closure:
let nx_p = nx_ptr.0;
vst1q_f32(nx_p.add(r * cols + c), result);
```

Use `move` on the closure to copy `SendPtr` values in by value (not reference). `move` also copies `&Heightmap` (references are `Copy`) and `f32` scalars.

---

## 9. Benchmarking: Cache Warmup

**The problem**: sequential benchmarks — the second run gets warm cache from the first run's reads. A 26 MB heightmap warm in L3 saves ~1ms on a ~5ms benchmark.

**Eviction buffer** between runs:
```rust
let evict: Vec<i32> = (0..80 * 1024 * 1024).map(|i| i as i32).collect();
std::hint::black_box(evict);  // 320 MB, flushes reads (26 MB) + outputs (147 MB)
```

**Most reliable**: run each variant in isolation (single benchmark per process invocation).

---

## 10. Performance Results

All numbers: M4 Max, cold cache, isolated runs.

| Implementation | GB/s | vs Scalar |
|---|---|---|
| True scalar (black_box) | 8.1 | 1× |
| Auto-vectorized "scalar" | 24.3 | 3.0× |
| NEON 4-wide explicit | 28.8 | 3.6× |
| NEON 8-wide (unrolled) | 27.5 | 3.4× |
| NEON parallel (10 cores) | 50.5 | 6.2× |
| NEON parallel (warm cache) | ~117 | — |

**8-wide slower than 4-wide**: register pressure. 8-wide needs ~24 live SIMD registers simultaneously; NEON has 32 total. Approaching the limit causes spills. M4's OOO engine already fills the pipeline at 4-wide.

**Parallel scaling: 1.75× from 10 cores (cold)**. Bottleneck: DRAM write bandwidth. 147 MB of output exceeds L3 (16 MB). All cores share the same memory bus. No benefit from more cores once bandwidth-saturated.

**Parallel scaling: ~2.5–4× (warm)**. L3 bandwidth is higher and scales better across cores.

**Write-allocate overhead**: fresh store to an uncached line triggers read-for-ownership first, doubling write traffic. Actual DRAM utilization ≈ 2× the "useful" write count.

---

## 11. Data Sizes (N47, 3601×3601)

| Data | Size |
|---|---|
| Heightmap `Vec<i16>` | 26 MB |
| One SoA array `Vec<f32>` | 49 MB |
| Full NormalMap (3 arrays) | 156 MB |
| Read + write per normal pass | ~260 MB |
| Eviction buffer needed | ~320 MB |

---

## 12. Tiled Normal Computation

Implemented `compute_normals_neon_tiled` and `compute_normals_neon_tiled_parallel` in `crates/terrain/src/tiled.rs`. Motivation: upper/lower neighbor stride in tiled layout = `tile_size * 2 = 256 bytes` vs `cols * 2 = 7202 bytes` in row-major.

**Key implementation differences**:
- Outer loops over tiles: `for tr in 0..tile_rows { for tc in 0..tile_cols { ... } }`
- Tile pointer: `tiles.as_ptr().add((tr * tile_cols + tc) * tile_size²)`
- Neighbor access: `tile_ptr.add((local_r ± 1) * tile_size + local_c)` — stride `tile_size` not `cols`
- Cross-tile boundaries skipped: process `local_r in 1..tile_size-1` only (top/bottom row of each tile left as zero)
- Output: `global_r * cols + global_c` (still row-major SoA)
- Parallel: `(0..tile_rows * tile_cols).into_par_iter()`, decompose `tile_idx` → `tr`, `tc`

**Results (isolated cold, M4 Max)**:

| Implementation | GB/s |
|---|---|
| Row-major NEON single-thread | 24.1 |
| Tiled NEON single-thread | 22.9 |
| Row-major NEON parallel | 42.3 |
| Tiled NEON parallel | 34.0 |

**Tiled is slower in both cases.**

Single-thread (~5%): M4 prefetcher handles 7202-byte stride well. Extra overhead (coordinate math, boundary guards) cancels the input locality gain.

Parallel (19%): Output is still row-major. Tile (tr, tc) writes to rows `[tr*128..(tr+1)*128]` × cols `[tc*128..(tc+1)*128]` — consecutive rows within a tile are 14404 bytes apart in the output. Multiple threads writing scattered column ranges hurt write-allocate efficiency more than row-major parallel's contiguous per-thread output ranges.

**The asymmetry**: tiled reduces *read* stride (256 vs 7202 bytes), but the *output* is 6× larger than the input (156 MB vs 26 MB). Writes dominate bandwidth, and tiled makes them worse.

**When tiled would win**: diagonal access patterns (shadow sweep with non-horizontal sun), or if output is also stored in tiled layout.

---

## 13. Rust 2021 Precise Disjoint Capture

Rust 2021 edition changed closure capture from "capture the variable" to "capture the minimal path accessed."

```rust
struct SendPtr(*mut f32);
unsafe impl Send for SendPtr {}
unsafe impl Sync for SendPtr {}

// BROKEN (Rust 2021): captures nx_ptr.0 (*mut f32) — SendPtr's Sync bypassed
vst1q_f32(nx_ptr.0.add(out), ...);

// FIXED: captures nx_ptr (SendPtr) — Sync holds
impl SendPtr {
    fn get(&self) -> *mut f32 { self.0 }
}
vst1q_f32(nx_ptr.get().add(out), ...);
```

`nx_ptr.0` is a field-path expression. The closure captures the field type `*mut f32`, not the struct `SendPtr`. `unsafe impl Sync for SendPtr {}` is irrelevant because `SendPtr` itself is not captured. Calling `.get()` forces the closure to capture `nx_ptr: SendPtr`, which is `Sync`.

---

## 14. Open Items for Next Phases

- `fill_nodata` division-by-zero if all 4 directions hit boundary without valid data
- Phase 1 `profiling::timed` label bug: `random_read`, `seq_write`, `random_write` all use label `"seq_read"`
- Phase 3 shadow sweep will also be write-heavy — same bandwidth ceiling applies
- Tiled normal computation leaves cross-tile boundary pixels as zero (incorrect for those rows) — acceptable for now, would require a halo-exchange pass to fix properly
