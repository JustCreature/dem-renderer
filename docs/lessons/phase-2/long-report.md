# Phase 2: Normal Computation — SIMD, SoA Layout, and Rayon Parallelism
## Comprehensive Student Textbook

---

## Part 1: What Are Surface Normals and Why Do We Need Them?

### 1.1 The Lighting Problem

When rendering terrain, we need to know how bright each pixel should be. Brightness depends on one thing: **how directly is the light (sun) hitting the surface at that point?**

A surface facing directly toward the sun gets maximum brightness. A surface tilted 90° away from the sun gets zero brightness. A surface facing away from the sun is in shadow.

The mathematical way to express "how much is this surface facing the sun" is:

```
brightness = cos(angle between surface orientation and sun direction)
```

If angle = 0° → cos = 1.0 → full brightness
If angle = 90° → cos = 0.0 → no light
If angle > 90° → cos < 0.0 → clamp to 0 (facing away)

This model is called **Lambertian shading** and is physically accurate for diffuse surfaces (rock, snow, grass).

### 1.2 What Is a Normal Vector?

A **normal vector** at a point on a surface is a vector that points perpendicularly away from that surface. It encodes the *orientation* of the surface — which direction it faces.

- Flat ground → normal points straight up: `(0, 0, 1)`
- Slope tilted right → normal leans left: `(-0.7, 0, 0.7)`
- Slope tilted toward you → normal leans away from you

For a heightmap, we compute one normal per pixel. The result is a `NormalMap` — one (nx, ny, nz) triple per pixel encoding surface orientation.

### 1.3 Why Normalize?

The normal computation produces a vector pointing in the right direction, but its *length* depends on slope steepness. A steep slope produces a long normal vector; flat terrain produces a short one.

For the lighting formula `dot(normal, sun) = cos(angle)` to work correctly, both vectors must be **unit length** (`|v| = 1`). If the normal isn't unit length, the dot product gives `|normal| × |sun| × cos(angle)` — wrong by a factor proportional to slope steepness.

**Normalization** divides the vector by its own length:

```
length = sqrt(nx² + ny² + nz²)
nx /= length
ny /= length
nz /= length
```

After normalization, `|normal| = 1` and only direction remains.

---

## Part 2: Computing Normals from a Heightmap

### 2.1 Finite Differences

We have a discrete grid of elevation values `h[r][c]`. We approximate the partial derivatives using **central differences**:

```
∂h/∂x ≈ (h[r][c+1] - h[r][c-1]) / (2 * dx)
∂h/∂y ≈ (h[r+1][c] - h[r-1][c]) / (2 * dy)
```

The surface normal is the gradient of the height function:

```
nx = (h[r][c-1] - h[r][c+1]) / (2 * dx_meters)   ← note: left - right
ny = (h[r-1][c] - h[r+1][c]) / (2 * dy_meters)   ← north - south
nz = 1.0
```

Then normalize `(nx, ny, nz)`.

**Why left minus right (not right minus left)?**

By convention, the normal points outward from the surface. If terrain rises going right (east), the surface tilts right, so the normal should lean left (negative x). `left - right = h[c-1] - h[c+1]`: if `h[c+1] > h[c-1]` (rising right), this gives negative nx — correctly pointing left. Flipping it would make normals point into the terrain.

### 2.2 Cell Sizes in Meters (Geo 101)

The heightmap uses SRTM-1 data: 1 arc-second per pixel. But pixels are not square on the ground:

- `dy_meters ≈ YDIM_degrees × 111,320` (latitude, ~constant)
- `dx_meters ≈ XDIM_degrees × 111,320 × cos(lat)` (longitude, shrinks with latitude)

For the N47 tile (47°N): `dy ≈ 30.9 m`, `dx ≈ 21.1 m`.

Using `dx = dy = 1.0` produces geometrically wrong normals — slopes appear steeper east-west than they actually are. The correct implementation uses the real cell sizes from `hm.dx_meters` and `hm.dy_meters`.

### 2.3 Boundary Handling

The central difference for pixel `(r, c)` reads `r-1`, `r+1`, `c-1`, `c+1`. The border pixels have no valid neighbors in one direction. The simple solution: process only interior pixels `(1..rows-1, 1..cols-1)` and leave the border pixels as zeros.

---

## Part 3: Memory Layout — AoS vs SoA

### 3.1 Array of Structs (AoS)

```rust
struct Normal { nx: f32, ny: f32, nz: f32, _pad: f32 }
let normals: Vec<Normal> = vec![...]; // 16 bytes per pixel
```

Memory layout:
```
[nx0 ny0 nz0 pad | nx1 ny1 nz1 pad | nx2 ny2 nz2 pad | ...]
```

To load 4 consecutive `nx` values for SIMD, you must load 4 full structs (64 bytes) and extract the `nx` lanes — 4 loads + shuffles before any math.

### 3.2 Struct of Arrays (SoA)

```rust
struct NormalMap {
    nx: Vec<f32>,  // all nx values contiguous
    ny: Vec<f32>,  // all ny values contiguous
    nz: Vec<f32>,  // all nz values contiguous
    rows: usize,
    cols: usize,
}
```

Memory layout:
```
nx: [nx0 nx1 nx2 nx3 nx4 nx5 nx6 nx7 ...]
ny: [ny0 ny1 ny2 ny3 ...]
nz: [nz0 nz1 nz2 nz3 ...]
```

Loading 4 consecutive `nx` values: one 16-byte aligned load, zero shuffles. Every byte is useful.

### 3.3 Cache Line Utilization

A 64-byte cache line holds:
- AoS: 4 full normals — but each load for `nx`-only operations wastes 12 of 16 bytes
- SoA `nx` array: 16 floats (16 pixels' `nx`) — 100% utilization when operating on one field

**SoA gives 16× more useful data per cache line** when making a single-field pass (e.g., computing dot products in the shading pass). This project uses SoA.

---

## Part 4: Pre-Implementation Reasoning

### 4.1 Memory Bandwidth Analysis

**Writes**: 3 SoA arrays × 4 bytes × rows × cols = 3 × 4 × 3601² ≈ 156 MB
**Reads**: 4 neighbors × 2 bytes × (rows-2) × (cols-2) ≈ 104 MB
**Total**: ~260 MB

At 20 GB/s DRAM write bandwidth: 156 MB / 20 GB/s ≈ 7.8ms for writes alone.

**Compute estimate** (normalize, NEON 4-wide): ~12 NEON instructions per 4 pixels, ~16M pixels / 4 = 4M iterations. At ~8 GOp/s effective: ~6ms.

Conclusion: write-bandwidth and compute are in the same order — neither dominates wildly. Actual measurement needed.

### 4.2 Write Pattern Analysis

When processing tile `(tr, tc)` from the tiled heightmap into the SoA output (row-major):

- **Within a tile row**: writes to `nx[r * cols + c .. c+ts]` — stride 4 bytes, prefetcher loves this
- **Between tile rows**: jump of `cols * 4 = 3601 * 4 ≈ 14 KB` — new stream per array

With 3 SoA arrays and only 3 active write streams simultaneously, the hardware stream detector (capacity ~8–16 streams) handles this fine. The within-row portion is sequential and maximally prefetcher-friendly.

---

## Part 5: Scalar Baseline Implementation

### 5.1 Code Structure

```rust
pub fn compute_normals_scalar(hm: &dem_io::Heightmap) -> NormalMap {
    let mut nx = vec![0.0f32; hm.rows * hm.cols];
    let mut ny = vec![0.0f32; hm.rows * hm.cols];
    let mut nz = vec![0.0f32; hm.rows * hm.cols];

    for r in 1..hm.rows - 1 {
        for c in 1..hm.cols - 1 {
            let upper = hm.data[(r - 1) * hm.cols + c] as f32;
            let lower = hm.data[(r + 1) * hm.cols + c] as f32;
            let left  = hm.data[r * hm.cols + (c - 1)] as f32;
            let right = hm.data[r * hm.cols + (c + 1)] as f32;

            let single_nx = (left - right) / (2.0 * hm.dx_meters as f32);
            let single_ny = (upper - lower) / (2.0 * hm.dy_meters as f32);
            let single_nz = 1.0f32;

            let length = f32::sqrt(
                single_nx * single_nx + single_ny * single_ny + single_nz * single_nz
            );

            nx[r * hm.cols + c] = single_nx / length;
            ny[r * hm.cols + c] = single_ny / length;
            nz[r * hm.cols + c] = single_nz / length;
        }
    }
    NormalMap { nx, ny, nz, rows: hm.rows, cols: hm.cols }
}
```

### 5.2 The Auto-Vectorization Surprise

**Measured result**: 24.3 GB/s (isolated, cold cache)

Checking the assembly:
```sh
cargo rustc -p terrain --release -- --emit=asm && \
  grep -i "fsqrt\|sqrtps\|sqrtpd" target/release/deps/terrain-*.s
```

Output:
```
fsqrt.4s   v17, v17
fsqrt.4s   v16, v16
fsqrt      s7, s7
```

The compiler **auto-vectorized the loop** — LLVM saw the pattern, determined it was safe to batch 4 iterations, and emitted:
- `fsqrt.4s`: 4-lane NEON sqrt (8 pixels per iteration — two registers unrolled)
- `fsqrt s7, s7`: scalar tail cleanup

This means what appeared to be a "scalar" baseline was actually already running NEON. To get a true scalar number, `std::hint::black_box` was used to break the vectorizer — yielding **8.1 GB/s** (scalar) vs **24.3 GB/s** (auto-vectorized).

**Key lesson**: LLVM is aggressive. Check assembly before claiming "scalar" benchmarks. Auto-vectorization is real and significant (3× here), but it's fragile — a branch or non-contiguous access can silently disable it.

---

## Part 6: Explicit NEON Implementation

### 6.1 Why Write Explicit NEON?

Auto-vectorization uses `fsqrt.4s`. Explicit NEON can use `vrsqrteq_f32` (reciprocal sqrt estimate) + Newton-Raphson refinement instead — potentially faster, and you control exactly what instructions are emitted.

Additionally, auto-vectorization may not always trigger. Explicit NEON is robust.

### 6.2 NEON Type System

On AArch64:
- `int16x4_t` — 64-bit register, 4 × i16
- `int32x4_t` — 128-bit register, 4 × i32
- `float32x4_t` — 128-bit register, 4 × f32

Key intrinsics used:
| Intrinsic | Operation |
|---|---|
| `vld1_s16(ptr)` | Load 4 × i16 from memory |
| `vmovl_s16(v)` | Widen i16x4 → i32x4 (sign-extend) |
| `vcvtq_f32_s32(v)` | Convert i32x4 → f32x4 |
| `vsubq_f32(a, b)` | a - b, elementwise |
| `vmulq_f32(a, b)` | a × b, elementwise |
| `vaddq_f32(a, b)` | a + b, elementwise |
| `vdupq_n_f32(x)` | Splat scalar x into all 4 lanes |
| `vrsqrteq_f32(v)` | Reciprocal sqrt estimate (~8-bit accurate) |
| `vrsqrtsq_f32(a, b)` | Newton-Raphson step: `(2 - a*b) / 2` |
| `vst1q_f32(ptr, v)` | Store 4 × f32 to memory |

### 6.3 Loading 4 Neighbors for 4 Pixels at Once

For pixels at columns `c, c+1, c+2, c+3`:

```
upper: hm.data[(r-1)*cols + c .. c+4]  — 4 contiguous i16s
lower: hm.data[(r+1)*cols + c .. c+4]  — 4 contiguous i16s
left:  hm.data[r*cols + c-1 .. c+3]    — 4 contiguous i16s, offset -1
right: hm.data[r*cols + c+1 .. c+5]    — 4 contiguous i16s, offset +1
```

After `vsubq_f32(left, right)`:
- Lane 0: `h[r][c-1]   - h[r][c+1]`   ← nx for pixel c
- Lane 1: `h[r][c]     - h[r][c+2]`   ← nx for pixel c+1
- Lane 2: `h[r][c+1]   - h[r][c+3]`   ← nx for pixel c+2
- Lane 3: `h[r][c+2]   - h[r][c+4]`   ← nx for pixel c+3

The lane alignment is automatic — each lane independently computes the correct central difference for its own pixel. This is the core elegance of SIMD for stencil operations.

### 6.4 Reciprocal Sqrt + Newton-Raphson

**The problem with `fsqrt.4s`**: on most architectures, sqrt is iterative and cannot be fully pipelined. Throughput ≈ latency.

**The solution**: use a table-lookup estimate and refine with multiply-only iterations.

`vrsqrteq_f32(x)` computes a ~8-bit accurate estimate of `1/sqrt(x)` using a hardware lookup table. One Newton-Raphson step refines to ~24-bit accuracy (sufficient for f32):

```rust
let est = vrsqrteq_f32(len_sq);
// vrsqrtsq_f32(a, b) computes (2 - a*b) / 2
let refined = vmulq_f32(
    vrsqrtsq_f32(vmulq_f32(len_sq, est), est),
    est,
);
// refined ≈ 1/sqrt(len_sq)
```

Then multiply instead of divide:
```rust
nx_out = vmulq_f32(vec_nx, refined);  // nx / sqrt(len_sq)
```

**Why this is faster than `fsqrt`**: the entire sequence is multiplies (`vmul`, `vrsqrts`), which have 1-cycle throughput vs fsqrt's ~3-10 cycle throughput. On M4 specifically, `fsqrt.4s` is relatively fast (~3-4 cycles), so the advantage is smaller than on x86.

### 6.5 Key Bug: `vdivq_f32` is Slow

Initial NEON implementation used `vdivq_f32` to divide by `2 * dx_meters` inside the inner loop:

```rust
// SLOW: division inside hot loop
let vec_nx = vdivq_f32(vsubq_f32(left, right), vdupq_n_f32(2.0 * dx));
```

**Measured**: 18.3 GB/s cold (worse than scalar).

`2 * dx_meters` is **loop-invariant** — it never changes. The compiler didn't hoist it because the code was written with intrinsics. Fix: precompute the reciprocal once and use multiplication:

```rust
// Outside both loops — computed once
let inv_2dx = vdupq_n_f32(1.0 / (2.0 * hm.dx_meters as f32));
let inv_2dy = vdupq_n_f32(1.0 / (2.0 * hm.dy_meters as f32));

// Inside inner loop — multiply instead of divide
let vec_nx = vmulq_f32(vsubq_f32(left, right), inv_2dx);
```

Division throughput: ~10–15 cycles. Multiply throughput: **1 cycle**. This fix alone: 18.3 → 28.8 GB/s.

**Key lesson**: division is structurally different from multiplication. Multiplication uses a fixed parallel grid of AND/ADD gates. Division is digit-by-digit iterative — each quotient digit depends on the previous one. No pipelining across the same operation. Always replace `/ constant` with `* (1/constant)` in hot loops.

### 6.6 `unsafe` Requirements

NEON intrinsics require `unsafe`:
1. They call `core::arch::aarch64` functions — platform-specific, not portable
2. The safety invariant (AArch64 with NEON, valid pointer bounds) cannot be verified by the compiler
3. Misuse (wrong pointer, wrong register interpretation) causes undefined behavior

The function is declared `pub unsafe fn`. Callers wrap the call in `unsafe {}`.

**Important**: `unsafe {}` blocks do **not** propagate into closure bodies. A closure defined inside an `unsafe {}` block is still a safe closure — it needs its own inner `unsafe {}` to call intrinsics.

---

## Part 7: The Cache Warmup Problem in Benchmarking

### 7.1 What Happened

Running scalar first, then NEON showed:
- Scalar: 24.3 GB/s
- NEON: 38–47 GB/s

This looked like a 2× NEON win. But the comparison was unfair: scalar ran cold (DRAM reads), NEON ran warm (heightmap already in L3 from scalar run).

### 7.2 Why Cache Warmup Matters So Much Here

The heightmap (26 MB reads) + output arrays (147 MB writes) = 173 MB of data. The output arrays are freshly allocated each time, so they're always cold. But reads from `hm.data` are shared — once the first function reads them, they sit in L3 for the second function.

Warming 26 MB of reads at ~24 GB/s saves ~1ms — which is significant when the whole benchmark takes ~5–10ms.

### 7.3 Getting Clean Numbers

**Wrong approach**: run benchmarks back-to-back without eviction.

**Right approach**: allocate and touch a large eviction buffer between runs:
```rust
let evict: Vec<i32> = (0..80 * 1024 * 1024).map(|i| i as i32).collect();
std::hint::black_box(evict);
```

This must be large enough to flush: reads (26 MB) + previous run's output (147 MB) ≈ 320 MB. Use ~320 MB of i32 = 80M elements.

For maximum reliability: run each variant in isolation (only one benchmark per execution).

---

## Part 8: Rayon Parallelism

### 8.1 The Ownership Problem

`rayon` requires closures to be `Send + Sync`. When multiple threads write to `nx`, `ny`, `nz`, Rust's borrow checker rejects multiple simultaneous `&mut Vec<f32>` — it can't prove at compile time that the threads write to non-overlapping indices.

### 8.2 The SendPtr Pattern

Solution: bypass the borrow checker with raw pointers wrapped in a newtype that manually asserts thread safety:

```rust
struct SendPtr(*mut f32);
unsafe impl Send for SendPtr {}
unsafe impl Sync for SendPtr {}
```

- `Send`: safe to transfer ownership to another thread
- `Sync`: safe to share a reference (`&T`) across threads
- `unsafe impl`: YOU take responsibility for the invariant: "no two threads access the same memory location simultaneously"

The invariant is maintained because each thread `r` only writes to index `r * cols + c` in the output arrays, and all `r` values from `into_par_iter` are distinct.

### 8.3 SIMD Types Are Not Sync

`float32x4_t` internally contains `*mut f32` in the AArch64 representation, making it `!Sync`. Attempting to capture a `float32x4_t` by reference across thread boundaries (non-move closure) fails:

```
error: `*mut f32` cannot be shared between threads safely
```

Fix: capture scalar `f32` values that are `Sync`, compute the NEON constants inside the closure (each thread creates its own register):

```rust
let inv_2dx_f32 = 1.0f32 / (2.0 * hm.dx_meters as f32);  // f32: Sync

(1..hm.rows - 1).into_par_iter().for_each(move |r| {
    unsafe {
        let inv_2dx = vdupq_n_f32(inv_2dx_f32);  // local to this thread
        // ...
    }
});
```

`vdupq_n_f32` is one instruction — negligible cost to call it once per row.

### 8.4 move Closures

The `move` keyword makes the closure capture variables by value instead of by reference. This is necessary when:
1. The captured variables contain types (like `SendPtr`) that are `Send + Sync` by value but not by reference due to transitivity
2. The closure will outlive the current stack frame (as rayon closures do)

### 8.5 Unsafe Closures

In Rust stable, closures cannot be declared `unsafe fn`. To call unsafe functions inside a closure, wrap them in `unsafe {}` inside the closure body:

```rust
.for_each(move |r| {
    unsafe {  // required — outer unsafe{} does NOT propagate into closure bodies
        let v = vld1_s16(ptr);
        // ...
    }
});
```

---

## Part 9: Performance Results and Analysis

### 9.1 Full Results Table (cold cache, isolated runs)

| Implementation | GB/s | vs Scalar |
|---|---|---|
| Scalar (with black_box) | 8.1 | baseline |
| Scalar (auto-vectorized) | 24.3 | 3.0× |
| NEON 4-wide explicit | 28.8 | 3.6× |
| NEON 8-wide (unrolled) | 27.5 | 3.4× |
| NEON parallel (10 cores) | 50.5 | 6.2× |

### 9.2 Why 8-Wide is Slower Than 4-Wide

Unrolling doubles the number of live SIMD registers per iteration. With 8-wide, ~12 registers are active per group × 2 groups = ~24 live registers simultaneously. NEON has 32 128-bit registers. When register pressure approaches the limit, the compiler spills to the stack (extra load/store instructions). Additionally, the M4's out-of-order engine was already keeping the pipeline full at 4-wide, so unrolling added complexity without adding throughput.

### 9.3 Why Rayon Gives Only 1.75× Over Single-Thread

With 10 performance cores on M4 Max, ideal scaling would be 10×. Actual scaling: 1.75×.

The bottleneck is **DRAM write bandwidth**. The 147 MB of output (nx, ny, nz) cannot be absorbed by L3 (16 MB on M4 Max). All 10 cores must write to DRAM, sharing the same memory bus. Adding more cores doesn't add more memory bus bandwidth — it just means more threads competing for the same resource.

**Amdahl's law for memory-bound workloads**: `speedup = bandwidth / per_thread_demand`. Beyond the bandwidth ceiling, adding threads provides no benefit.

### 9.4 Warm Cache Tells the Compute Story

| Implementation | Warm cache GB/s |
|---|---|
| NEON single-thread | ~41–47 |
| NEON parallel | ~117 |

When reads hit L3 instead of DRAM, the parallel version shows ~2.5–4× scaling — much better than cold 1.75×. This is because L3 bandwidth is higher than DRAM bandwidth and scales better across cores.

117 GB/s warm parallel represents the true compute throughput ceiling of this implementation on M4 Max.

### 9.5 The Write-Allocate Problem

Our measured "useful data" bandwidth of 50.5 GB/s corresponds to ~173 MB. But the actual DRAM traffic is higher due to **write-allocate**: when a store misses the cache, the CPU must first fetch the cache line (read-for-ownership) before writing to it. This effectively doubles write traffic for fresh allocations. The actual DRAM utilization is ~100–150 GB/s, against M4 Max's 546 GB/s theoretical peak.

The remaining gap from peak is explained by the strided write pattern (14 KB jumps between tile rows in the SoA output), which limits write coalescing efficiency in the DRAM controller.

---

## Part 10: Tiled Normal Computation — Does Tiling Help?

### 10.1 The Hypothesis

Row-major normal computation reads upper and lower neighbors with stride `cols * 2 = 7202` bytes between them. The tiled heightmap stores pixels in 128×128 blocks. Within a tile, the upper neighbor is only `tile_size * 2 = 256` bytes away. If the prefetcher struggles with 7202-byte strides, tiling should help.

### 10.2 Implementation

The tiled functions live in `crates/terrain/src/tiled.rs`.

**Outer structure** (single-threaded):
```rust
for tr in 0..hm.tile_rows {
    for tc in 0..hm.tile_cols {
        let tile_start = (tr * hm.tile_cols + tc) * hm.tile_size * hm.tile_size;
        let tile_ptr = hm.tiles().as_ptr().add(tile_start);

        for local_r in 1..hm.tile_size - 1 {
            // upper/lower neighbor stride: tile_size (256 bytes), not cols (7202 bytes)
            let ptr_upper = tile_ptr.add((local_r - 1) * hm.tile_size + local_c);
            let ptr_lower = tile_ptr.add((local_r + 1) * hm.tile_size + local_c);
            ...
            // output: global coordinates in row-major output
            let out = global_r * hm.cols + global_c;
        }
    }
}
```

**Cross-tile boundary handling**: the loop processes `local_r in 1..tile_size-1`, skipping the first and last row of every tile. These rows would need neighbor data from an adjacent tile, which is not contiguous. The border pixels are left as zero in the output. This means ~2/128 ≈ 1.6% of rows are incorrect (missing normals at tile seams). Acceptable for now; fixing it properly would require a halo-exchange pass.

**Parallel version**: parallelizes over tiles instead of rows:
```rust
(0..tile_rows * tile_cols).into_par_iter().for_each(move |tile_idx| {
    let tr = tile_idx / tile_cols;
    let tc = tile_idx % tile_cols;
    unsafe { ... }  // same tile processing logic
});
```

### 10.3 Bug: Rust 2021 Precise Disjoint Capture

The parallel version failed to compile:
```
error: `*mut f32` cannot be shared between threads safely
```

This is a Rust 2021 edition change: **closures capture the minimal path they access, not the whole variable**.

```rust
struct SendPtr(*mut f32);
unsafe impl Send for SendPtr {}
unsafe impl Sync for SendPtr {}

// BROKEN: accessing .0 makes the closure capture the field nx_ptr.0 (*mut f32)
// SendPtr's unsafe impl Sync is irrelevant — the captured type is *mut f32, not SendPtr
vst1q_f32(nx_ptr.0.add(out), ...);
```

The fix is a `get()` method that forces the closure to capture `nx_ptr` (the full `SendPtr`) rather than `nx_ptr.0` (the raw `*mut f32`):

```rust
impl SendPtr {
    fn get(&self) -> *mut f32 { self.0 }
}

// FIXED: closure captures nx_ptr: SendPtr — Sync holds
vst1q_f32(nx_ptr.get().add(out), ...);
```

In Rust 2018 and earlier, `nx_ptr.0` and `nx_ptr.get()` were equivalent for capture purposes — both captured the whole struct. Rust 2021's "precise disjoint capture" makes them different: field access captures the field's type directly.

**Note**: the row-major parallel version had already hit this bug and was fixed earlier. The tiled version was written with `.0` and needed the same fix.

### 10.4 Results (isolated cold, M4 Max)

| Implementation | GB/s |
|---|---|
| Row-major NEON single-thread | 24.1 |
| Tiled NEON single-thread | 22.9 |
| Row-major NEON parallel | 42.3 |
| Tiled NEON parallel | 34.0 |

**Tiled is slower in both cases.**

### 10.5 Why Tiled Is Slower Single-Threaded

Phase 1 showed that the M4's prefetcher handles the 7202-byte stride in row-major neighbor-sum at 39 GB/s. The prefetcher's stream detectors can track multiple parallel streams with large but constant strides. The stride is predictable, so hardware prefetch pipelines the loads effectively.

Tiled adds overhead without a matching gain:
- Global coordinate arithmetic per pixel: `global_r = tr * tile_size + local_r`
- Extra boundary guards: `if global_r >= rows - 1`, `if global_c + 4 >= cols`
- Skipped border rows (slightly less total work, but the iteration structure is more complex)

Net result: input locality gain ≈ input locality overhead. Roughly equal performance (~5% tiled slower).

### 10.6 Why Tiled Is Worse in Parallel

This is the more important result. Tiled parallel is 19% slower than row-major parallel.

**Row-major parallel output pattern**: thread N handles rows `[N·chunk, (N+1)·chunk)`. Its writes to `nx`, `ny`, `nz` sweep through a **contiguous region** — addresses increase by 4 bytes between consecutive writes within a row, and by `cols * 4` bytes between rows. From the DRAM controller's perspective, each thread is doing one long sequential scan through its slice of the output arrays. Hardware write prefetchers train on this easily.

**Tiled parallel output pattern**: thread handling tile `(tr, tc)` writes to output rows `[tr·128, (tr+1)·128)` × cols `[tc·128, (tc+1)·128)`. Because the output is row-major (not tiled), consecutive rows within a tile are `cols * 4 = 14404` bytes apart. Consecutive tile writes from one thread interleave with the output ranges of other tiles. The write addresses jump around within the output arrays.

Ten threads simultaneously writing to non-contiguous, interleaved column ranges means:
- More write-allocate cache line fetches landing on unpredictable lines
- Less effective write coalescing in the DRAM controller
- Prefetcher cannot predict the next write address as reliably

### 10.7 The Core Asymmetry

The tiling optimization is asymmetric:

- **Input** (heightmap i16): 26 MB. Tiled layout reduces neighbor stride 7202 → 256 bytes. Benefit: better prefetch for reads.
- **Output** (NormalMap f32 SoA): 156 MB. Still row-major. Tiled iteration order makes writes scattered.

The output is **6× larger** than the input. For a write-bandwidth-bound workload (which this is — 147 MB of output exceeds L3), making writes harder to prefetch costs more than better read locality saves.

**When tiled would actually win**:
1. If the output were also stored in tiled layout (`TiledNormalMap` with tiled SoA) — writes become sequential within each tile, matching the compute pattern.
2. For diagonal access patterns (shadow sweep with a non-horizontal sun): row-major layout forces diagonal reads that jump `(cols ± 1) * 2` bytes per step, thrashing the cache. Tiled layout keeps diagonal neighbors in the same tile.

---

## Part 12: Key Lessons Summary

1. **Check assembly before claiming scalar numbers.** LLVM auto-vectorizes aggressively. Use `--emit=asm` and grep for `fsqrt`/`sqrtps`.

2. **Division inside hot loops is catastrophic.** `vdivq_f32` costs 10–15 cycles; `vmulq_f32` costs 1 cycle. Always hoist `/ constant` → `* (1/constant)` outside the loop.

3. **SoA enables SIMD.** AoS requires shuffles to gather a single field; SoA gives contiguous same-field data — one load, zero shuffles.

4. **SIMD for stencils works via window offset.** You don't load "the left neighbor for pixel 0, 1, 2, 3 separately." You load a window starting at `c-1` and another at `c+1`. Lane subtraction gives each pixel its correct neighbor.

5. **Unsafe does not propagate into closures.** Each closure body needs its own `unsafe {}` block.

6. **SIMD types are not Sync.** `float32x4_t` contains `*mut f32` internally. Use scalar `f32` for cross-thread captures; create NEON registers inside the closure.

7. **Rayon scales to the memory bandwidth ceiling, not the core count.** Cold cache, write-heavy workloads: expect 2–3× with 10 cores. Warm cache, compute-heavy: expect 4–8×.

8. **Benchmark order matters enormously.** Always evict cache between runs or run in isolation. The heightmap (26 MB) + outputs (147 MB) = 320 MB eviction needed.

9. **Tiling helps reads but hurts writes when the output is row-major.** Input locality gain from shorter neighbor strides (7202 → 256 bytes) is erased when output writes become scattered. Profile the dominant data direction first.

10. **Rust 2021 precise disjoint capture changes what closures capture.** `nx_ptr.0` captures `*mut f32` (bypassing `SendPtr`'s `Sync` impl). `nx_ptr.get()` captures `SendPtr` (impl holds). Always use method calls to force struct-level capture when thread safety depends on the wrapper type.

11. **Tiling wins for diagonal access patterns.** Row-major diagonal reads jump `(cols ± 1) * 2` bytes per step — the prefetcher sees a non-constant stride and gives up. Tiled layout keeps diagonal neighbors in the same tile (L1-resident). Phase 3 shadow sweep is the first place tiled layout should actually pay off.
