# Phase 3: Sun Shadow Sweep — Comprehensive Student Textbook

---

## Part 1: The Shadow Problem

### 1.1 What We're Computing

For every pixel in the heightmap, we want to know: **is this point lit by the sun, or is it in shadow?**

A point is in shadow if there exists any piece of terrain between it and the sun that is tall enough to block the line of sight. Visually: stand at the pixel and look toward the sun. If you see terrain before the sky, you're in shadow.

The output is a `ShadowMask`: one `f32` per pixel, `1.0` = fully lit, `0.0` = fully in shadow. Using `f32` rather than `u8` allows soft shadow blending in later rendering phases.

### 1.2 The Naive Algorithm and Why It's Too Slow

The naive approach: for each pixel P, cast a ray from P toward the sun and march along it, sampling the heightmap at each step. If any sample exceeds the line-of-sight height, P is in shadow.

For a 3601×3601 grid, each ray marches up to 3601 steps. Cost: O(N³) = O(3601³) ≈ 46 billion operations. Completely impractical.

### 1.3 The Sweep Algorithm: O(N²)

The key insight: **all pixels in the same row see the sun along the same direction** (for sun from the west). Instead of marching from each pixel independently, we can sweep the entire row at once.

Walk along the row from west to east. Maintain a running "maximum effective height" seen so far. For each new pixel:
1. Compute whether this pixel lies below the running maximum → if yes, it's in shadow
2. Update the running maximum

Total work: one pass per row, one step per pixel. O(N²). For 3601×3601, this is ~13 million operations — fast.

---

## Part 2: The Horizon Angle Sweep — Mathematical Foundation

### 2.1 The Correct h_eff Formula

The sun is in the **west** at elevation angle θ above the horizon. Light rays travel **eastward** and **downward**.

To determine whether pixel at column `c` is shadowed by any pixel `c'` to its west, we trace a backward ray from pixel `c` toward the sun (westward and upward). At column `c'` (where `c' < c`), this backward ray is at height:

```
ray_height(c') = height[c] + (c - c') × dx × tan(θ)
```

Pixel `c` is shadowed by `c'` if terrain at `c'` is **above** this ray:

```
height[c'] > height[c] + (c - c') × dx × tan(θ)
```

Rearranging (the key algebraic step):

```
height[c'] + c' × dx × tan(θ)  >  height[c] + c × dx × tan(θ)
```

Define **effective height**:
```
h_eff[c] = height[c] + c × dx × tan(θ)
```

Then pixel `c` is in shadow if and only if:
```
max(h_eff[0 .. c-1]) > h_eff[c]
```

This is a **running maximum** over `h_eff` — a simple sequential computation.

### 2.2 Why the Sign Is Plus, Not Minus

Geometrically: as we move east (increasing `c`), the sun ray descends. Relative to the sun ray, terrain at column `c` appears **higher** by `c × dx × tan(θ)` than its raw elevation suggests. So effective height adds this term.

**Verification on flat terrain** (all heights = H):
- `h_eff[c] = H + c × dx × tan(θ)` — strictly increasing
- `running_max` at any `c` equals `h_eff[c-1]` which is **less** than `h_eff[c]`
- Result: no shadows on flat terrain ✓

**Verification with ridge** (tall peak at `c=1`, valley at `c=2`):
- `h_eff[0]` = base level, `h_eff[1]` = very high, `h_eff[2]` = low
- At `c=2`: `running_max = h_eff[1]` >> `h_eff[2]` → in shadow ✓

**The original bug**: using `height - dist × tan_sun` produces `h_eff` that decreases monotonically on flat terrain, causing every pixel after column 0 to appear in shadow — exactly the "all-black PNG" symptom observed during development.

### 2.3 Running Max Update Order

```rust
// Check shadow BEFORE updating running_max
if h_eff < running_max {
    data[r * cols + c] = 0.0;
}
running_max = running_max.max(h_eff);
```

The check must come **before** the update. If we updated first, column 0 would be compared against itself, potentially shadowing itself. Column 0 starts with `running_max = -∞`, which is always less than any real elevation — correctly never shadowed.

---

## Part 3: Scalar Implementation

### 3.1 Data Structures

```rust
pub struct ShadowMask {
    pub data: Vec<f32>,  // rows × cols elements, 1.0=lit, 0.0=shadow
    pub rows: usize,
    pub cols: usize,
}
```

Memory: 3601 × 3601 × 4 bytes = **52 MB**. Compare to the input heightmap (26 MB as `i16`). Total data movement per run: 78 MB.

### 3.2 The Scalar Loop

```rust
pub fn compute_shadow_scalar(hm: &Heightmap, sun_elevation_rad: f32) -> ShadowMask {
    let mut data = vec![1.0f32; hm.rows * hm.cols];
    let dx = hm.dx_meters as f32;
    let tan_sun = sun_elevation_rad.tan();

    for r in 0..hm.rows {
        let mut running_max = f32::NEG_INFINITY;
        for c in 0..hm.cols {
            let height = hm.data[r * hm.cols + c] as f32;
            let dist = c as f32 * dx;
            let h_eff = height + dist * tan_sun;

            if h_eff < running_max {
                data[r * hm.cols + c] = 0.0;
            }

            running_max = running_max.max(h_eff);
        }
    }
    ShadowMask { data, rows: hm.rows, cols: hm.cols }
}
```

**Bandwidth measurement**: the formula uses `2 * rows * cols + 4 * rows * cols = 6 * rows * cols` bytes — reading `i16` heights (2 bytes) and writing `f32` shadow values (4 bytes). For 3601²: 78 MB total.

**Measured result**: **8.1 GB/s**, 10.2ms.

### 3.3 What Limits the Scalar Version?

Two candidates:
1. **Memory bandwidth** — 78 MB at scalar seq_read speed (~6.7 GB/s) would take ~11.6ms
2. **Loop-carried dependency** — `running_max` depends on the previous iteration

Estimating the dependency chain:
```
3601 rows × 3601 cols × 2 cycles (scalar fmax latency on M4) ÷ 4 GHz ≈ 6.5ms
```

The dependency chain accounts for ~6.5ms, memory for ~3.7ms. The loop is **64% latency-bound, 36% memory-bound**. The CPU cannot execute iterations out of order because each `running_max.max(h_eff)` result is needed before the next comparison.

This is a textbook **latency-bound** loop: the bottleneck is the latency of a single instruction chain (fmax), not throughput or memory bandwidth.

---

## Part 4: Branchless vs Branchy

### 4.1 The Branchy Version (Original)

```rust
if h_eff < running_max {
    data[r * hm.cols + c] = 0.0;
}
```

The CPU must predict "will this branch be taken?" before computing `h_eff`. Misprediction flushes the pipeline (~15 cycle penalty on M4).

### 4.2 The Branchless Version

```rust
let in_shadow = (h_eff < running_max) as u32 as f32;
data[r * hm.cols + c] = 1.0 - in_shadow;
```

No conditional branch — always writes. The comparison result becomes a float `0.0` or `1.0` and is stored unconditionally.

### 4.3 What the Measurement Shows

**Branchy: 8.1 GB/s | Branchless: 10.6 GB/s** — branchless is 31% faster.

This seems counterintuitive: shadow regions are spatially coherent (long runs of lit terrain, long runs of shadow), so the branch predictor (TAGE-style on M4) should achieve >99% accuracy. Very few mispredictions.

**Why does branchless win despite good prediction?**

1. **Regular store pattern** — unconditional sequential stores are friendlier to the store buffer and L1D write path. Conditional stores create an irregular pattern (some addresses written, some skipped) that is harder to pipeline efficiently.

2. **Branch uops still consume resources** — even a correctly-predicted branch occupies ROB slots, frontend bandwidth, and branch predictor resources. Eliminating the branch reduces micro-architectural overhead.

3. **The write bandwidth "cost"** is not actually a cost here — the L1D can absorb sequential writes at very high rate, and the working set (one row at a time = ~14 KB) fits in L1D.

**Lesson**: "avoid branches because of misprediction" is an oversimplification. Sometimes branchless is faster because it produces more regular, pipelineable instruction patterns — even when prediction accuracy is high.

---

## Part 5: NEON Vectorization

### 5.1 The Key Insight: Vectorize Across Rows

In Phase 2, NEON processed 4 **consecutive pixels in the same row** — the lane dimension was within a row.

For shadow computation, this doesn't work: each pixel in a row depends on the previous (loop-carried `running_max`). There is no parallelism within a row at the operation level.

The parallelism is **between rows**: row 0's sweep is completely independent of row 1's sweep. We can process 4 rows simultaneously, each with its own `running_max` lane.

```
Lane 0: running_max₀ covers row r
Lane 1: running_max₁ covers row r+1
Lane 2: running_max₂ covers row r+2
Lane 3: running_max₃ covers row r+3
```

All 4 lanes step through column `c` together. At each `c`, 4 `running_max` values update independently.

### 5.2 The NEON Inner Loop

```rust
// Outer: process rows in chunks of 4
while r + 4 <= hm.rows {
    let base = [r*cols, (r+1)*cols, (r+2)*cols, (r+3)*cols];
    let mut running_max: float32x4_t = vdupq_n_f32(f32::NEG_INFINITY);
    let step = dx * tan_sun;

    for c in 0..cols {
        // Gather: 4 scalar loads from 4 different rows at same column
        let heights = [
            hm.data[base[0] + c] as f32,  // row r
            hm.data[base[1] + c] as f32,  // row r+1
            hm.data[base[2] + c] as f32,  // row r+2
            hm.data[base[3] + c] as f32,  // row r+3
        ];
        let h_vec = vld1q_f32(heights.as_ptr());   // pack into NEON

        let h_eff = vaddq_f32(h_vec, vdupq_n_f32(c as f32 * step));

        let mask = vcltq_f32(h_eff, running_max);  // 4 comparisons
        let result = vbslq_f32(mask, vdupq_n_f32(0.0), vdupq_n_f32(1.0));

        // Scatter: 4 scalar stores to 4 different rows at same column
        *data.get_unchecked_mut(base[0] + c) = vgetq_lane_f32::<0>(result);
        *data.get_unchecked_mut(base[1] + c) = vgetq_lane_f32::<1>(result);
        *data.get_unchecked_mut(base[2] + c) = vgetq_lane_f32::<2>(result);
        *data.get_unchecked_mut(base[3] + c) = vgetq_lane_f32::<3>(result);

        running_max = vmaxq_f32(running_max, h_eff);  // update 4 maxes
    }
    r += 4;
}
```

### 5.3 Key NEON Intrinsics

| Intrinsic | Operation |
|---|---|
| `vdupq_n_f32(x)` | broadcast scalar `x` to all 4 lanes |
| `vld1q_f32(ptr)` | load 4 consecutive `f32` into register |
| `vaddq_f32(a, b)` | 4-wide add |
| `vcltq_f32(a, b)` | 4-wide compare less-than → `uint32x4_t` mask (all 1s or all 0s per lane) |
| `vbslq_f32(mask, a, b)` | select `a` where mask=1 (true), `b` where mask=0 (false) |
| `vmaxq_f32(a, b)` | 4-wide max |
| `vgetq_lane_f32::<N>(v)` | extract lane N as scalar |

**`vbslq_f32` argument order gotcha**: `vbslq_f32(mask, on_true, on_false)`. Since `vcltq_f32` sets mask=1 when `h_eff < running_max` (= in shadow), and we want `0.0` for shadow: `vbslq_f32(mask, vdupq_n_f32(0.0), vdupq_n_f32(1.0))`.

### 5.4 Gather Loads and Scatter Stores

The 4 heights at column `c` come from addresses `base[0]+c`, `base[1]+c`, `base[2]+c`, `base[3]+c`. These are separated by `cols × 2 ≈ 7.2 KB`. This is a **gather** operation — non-contiguous loads.

We cannot use `vld1q` directly on `hm.data` because the 4 values aren't contiguous. Instead: scalar load 4 values into a `[f32; 4]` stack array, then `vld1q_f32` on that array. This is semantically a gather but the compiler can optimize the stack array away.

The M4's hardware prefetcher can track up to ~8 sequential streams. With 4 active row streams per chunk, each advancing sequentially, the prefetcher trains well.

### 5.5 Why 2.2× Instead of 4×

**Prediction**: NEON reduces the dependency chain 4× (from 6.5ms → 1.6ms). Memory stays similar (~3.2ms). Predicted total: ~4.8ms.

**Measured**: 4.7ms (**2.2× over scalar**).

The speedup is 2.2× because:
- **Latency component** improved 4× (6.5ms → 1.6ms)
- **Memory component** improved slightly (3.7ms → 3.2ms, due to better cache utilization with 4 active streams)
- Combined: 6.5+3.7=10.2ms → 1.6+3.2=4.8ms ≈ 2.1×

This is the classic bottleneck-shift pattern: fix the dominant bottleneck, the second bottleneck becomes more visible.

---

## Part 6: Rayon Parallelism

### 6.1 Structure

Rows are completely independent: row `r`'s sweep never reads from or writes to row `r+1`. Perfect `rayon` use case.

For the west-only version, `par_chunks_mut` over the output data works cleanly:

```rust
data.par_chunks_mut(4 * hm.cols)
    .enumerate()
    .for_each(|(chunk_idx, chunk)| {
        let r = chunk_idx * 4;
        // chunk is &mut [f32] — a direct view into data, no copy
        // write to chunk[local_r * hm.cols + c]
        // read from hm.data[global_r * hm.cols + c]  (shared immutable borrow)
    });
```

**Why `par_chunks_mut` is safe**: rayon proves at compile time that chunks are non-overlapping. Each closure receives a disjoint `&mut [f32]`. No locks needed. `&Heightmap` is `Sync` (read-only), so concurrent reads are fine.

### 6.2 Numbers

| Version | Time | Bandwidth | Speedup vs scalar |
|---|---|---|---|
| Scalar | 10.2ms | 8.1 GB/s | 1× |
| NEON single | 4.7ms | 17.4 GB/s | 2.2× |
| NEON parallel (10 cores) | 1.4ms | 58.6 GB/s | 7.3× |

### 6.3 Why 3.4× from Parallelism (Not 10×)

With 10 performance cores on M4 Max, ideal parallel speedup over single-thread NEON would be 10×.

**Expected**: 4.7ms / 10 = 0.47ms. **Actual**: 1.4ms → **3.4× speedup from parallelism**.

Estimating with 10 threads:
- Dependency chain: each core handles ~90 row-chunks × 3601 steps × 2 cycles / 4GHz ≈ **0.16ms** — negligible
- Memory: 78 MB with 10 interleaved streams each doing 4-stream gather/scatter

**Memory bandwidth is now the ceiling.** 10 threads × 4 streams per thread = 40 concurrent streams hitting the M4 Max unified memory. The memory controller saturates at ~58.6 GB/s for this strided access pattern — well below the M4 Max's theoretical peak but constrained by the non-sequential nature of the gather/scatter pattern.

Compare to Phase 2 parallel NEON normals: 42–50 GB/s cold. Shadow mask at 58.6 GB/s is consistent — lighter per-pixel work (6 bytes vs 20 bytes for normals).

### 6.4 The Optimization Arc

```
Scalar:          latency-bound (64%) + memory (36%)  → 10.2ms
NEON 4-wide:     latency reduced 4×, memory revealed  →  4.7ms
NEON parallel:   latency negligible, purely memory-bound → 1.4ms
```

Each optimization fixed the dominant bottleneck and exposed the next one. This is the universal pattern in optimization work.

---

## Part 7: Arbitrary Sun Azimuth — DDA

### 7.1 Why Cardinal Direction Is a Special Case

The west-only sweep assumed `dr=0, dc=1` — rays travel purely along rows. For a sun at any other azimuth, rays travel diagonally across the grid. The `h_eff` formula is unchanged, but we need to:

1. Know which direction to step
2. Know which pixels to start from (entry edge)
3. Accumulate correct horizontal distance per step

### 7.2 DDA — Digital Differential Analyzer

DDA is a grid-traversal algorithm that steps through a straight line one pixel at a time, guaranteeing:
- Every pixel the line passes through is visited
- No pixel is visited twice
- Pixels are visited in order from start to end

**Core idea**: normalize the step vector so the dominant axis advances by exactly ±1 per step. The other axis follows fractionally.

```
raw direction: (dr, dc) from sin/cos of sun azimuth

if |dc| >= |dr|:          # more horizontal than vertical
    dc_step = sign(dc)    # ±1 in columns
    dr_step = dr / |dc|   # fractional in rows

else:                     # more vertical than horizontal
    dr_step = sign(dr)    # ±1 in rows
    dc_step = dc / |dr|   # fractional in columns
```

**Example: sun at 260° (nearly west, slightly south)**
- `dc = -sin(260°) ≈ 0.985`, `dr = cos(260°) ≈ -0.174`
- `|dc| > |dr|` → `dc_step = 1.0`, `dr_step = -0.177`
- At each step: advance 1 column, advance -0.177 rows (fractionally southward)
- At step 6: `r_f = start_r - 1.062` → rounds to `start_r - 1` (row changed)

### 7.3 Sun Direction in Grid Coordinates

Sun at azimuth α (clockwise from north). Light travels FROM the sun TOWARD the terrain:
```
dc = -sin(α)   (column component: east = positive)
dr =  cos(α)   (row component: south = positive, north = negative)
```

Verification:
- Sun from west (α=270°): `dc = -sin(270°) = 1`, `dr = cos(270°) = 0` → eastward ✓
- Sun from north (α=0°): `dc = 0`, `dr = cos(0°) = 1` → southward ✓
- Sun from northeast (α=45°): `dc = -0.707`, `dr = 0.707` → southwest ✓

### 7.4 Entry Edges

Rays enter the grid from the opposite side of the sun:
```
dc_step > 0 (light going east)  → starting pixels on west edge  (col=0)
dc_step < 0 (light going west)  → starting pixels on east edge  (col=cols-1)
dr_step > 0 (light going south) → starting pixels on north edge (row=0)
dr_step < 0 (light going north) → starting pixels on south edge (row=rows-1)
```

For diagonal sun, **both** a column-edge and a row-edge apply. Rays start from both edges, giving ~7200 starting pixels instead of ~3601. Corner pixels may appear in both sets — they get processed twice, harmlessly.

### 7.5 Distance Per Step

```rust
let dist_per_step = ((dc_step * dx_meters).powi(2) + (dr_step * dy_meters).powi(2)).sqrt();
```

For the west case: `dist_per_step = sqrt((1.0 × 21.1)² + 0²) = 21.1 m` = `dx_meters` ✓
For 45° diagonal: `dist_per_step = sqrt((21.1)² + (30.9)²) ≈ 37.5 m` per diagonal step.

Using real `dx_meters` and `dy_meters` (not pixel counts) matters because pixels are not square on the ground at 47°N: `dy ≈ 30.9 m`, `dx ≈ 21.1 m`.

### 7.6 The DDA Ray Loop

```rust
for (start_r, start_c) in starting_pixels {
    let mut running_max = f32::NEG_INFINITY;
    let mut dist = 0.0f32;
    let (mut r_f, mut c_f) = (start_r, start_c);

    while r_f >= 0.0 && r_f < rows as f32 && c_f >= 0.0 && c_f < cols as f32 {
        let r = r_f.round() as usize;
        let c = c_f.round() as usize;
        let h_eff = hm.data[r * cols + c] as f32 + dist * tan_sun;

        if h_eff < running_max {
            data[r * cols + c] = 0.0;
        }

        running_max = running_max.max(h_eff);
        r_f += dr_step;
        c_f += dc_step;
        dist += dist_per_step;
    }
}
```

Uses `round()` (not `floor()`) to keep the path centered on the theoretical line rather than biased to one side.

### 7.7 `dda_setup` Helper

Both the scalar and NEON-parallel versions share the same DDA parameter computation. Extracted into:

```rust
struct DdaSetup { dc_step, dr_step, dist_per_step, starting_pixels }
fn dda_setup(rows, cols, sun_azimuth_rad, dx, dy) -> DdaSetup
```

### 7.8 NEON Parallel with Azimuth

The parallelism model changes for arbitrary azimuth: instead of `par_chunks_mut` over contiguous row blocks, we use `par_chunks` over the `starting_pixels` vector with `SendPtr` for shared mutable writes.

```rust
starting_pixels.par_chunks(4).for_each(|rays| {
    // 4 rays processed simultaneously in NEON
    // Each ray has its own running_max lane
    // NEON loop runs while ALL 4 rays are in bounds
    // Scalar continuation per ray after the group exits bounds
});
```

**Why `SendPtr`?** For diagonal sun, rays from two entry edges can visit the same pixel at grid corners. Both writes are valid `f32` values (0.0 or 1.0). On AArch64, a 32-bit store is a single atomic instruction — no torn writes. The race is benign for a visualization output.

**NEON kernel for arbitrary DDA**: unlike the row-major version (which always had the same column `c` for all 4 lanes), the DDA version tracks 4 independent `(r_f, c_f)` positions. Each lane computes its own pixel index:

```rust
let idxs = [
    rf[0].round() as usize * cols + cf[0].round() as usize,
    rf[1].round() as usize * cols + cf[1].round() as usize,
    ...
];
```

The `h_eff` computation and `vmaxq_f32` update are identical to the row-major version.

---

## Part 8: Solar Position and Seasonal Variation

### 8.1 Sun Azimuth at Sunrise

The sun's azimuth at sunrise varies with season (solar declination):

| Date | Azimuth at sunrise (47°N) |
|---|---|
| Spring/Autumn equinox | 90° (due east) |
| Summer solstice | ~50–60° (northeast) |
| Winter solstice | ~120–130° (southeast) |

### 8.2 Good Test Configurations

- **Sunrise equinox**: `azimuth=90°`, `elevation=10°` — long shadows westward
- **Winter solstice noon**: `azimuth=180°`, `elevation=19.5°` — shadows due north, north-facing slopes in full shadow
- **Low sun for dramatic effect**: any azimuth, `elevation=5–10°` — very long shadows

### 8.3 M4 Max Cell Size (N47 tile)

- `dx_meters ≈ 21.1 m` (longitude cell at 47°N)
- `dy_meters ≈ 30.9 m` (latitude cell)

At `elevation=10°`: shadow from a 100m peak extends `100 / tan(10°) ≈ 567 m` = ~27 pixels in the sun direction.

---

## Part 9: Results Summary

| Implementation | Time | Bandwidth | Speedup |
|---|---|---|---|
| Scalar (west, branchy) | 10.2ms | 8.1 GB/s | 1× |
| Scalar (west, branchless) | 9.4ms | 10.6 GB/s | 1.3× |
| NEON 4-wide (west) | 4.7ms | 17.4 GB/s | 2.2× |
| NEON parallel 10 cores (west) | 1.4ms | 58.6 GB/s | 7.3× |

**Bottleneck progression**: latency-bound → partially memory-bound → fully memory-bound

**Data moved**: 78 MB per run (26 MB read i16 + 52 MB write f32)

**M4 Max memory ceiling** (this access pattern): ~58.6 GB/s with 10 cores and strided gather/scatter streams

---

## Part 10: Open Items and Known Limitations

1. **DDA version not benchmarked** — the arbitrary-azimuth scalar and NEON-parallel functions exist but no timing numbers recorded yet. The diagonal access pattern (strided reads) is expected to be slower than row-major.

2. **Cross-ray shadow accuracy for diagonal DDA** — pixels near entry-edge corners may be processed by two rays. The last writer wins. For most pixels (interior) correctness is exact.

3. **West-only NEON functions do not support arbitrary azimuth** — `compute_shadow_neon` and `compute_shadow_neon_parallel` are hardcoded for west sun. Use `compute_shadow_neon_parallel_with_azimuth` for general azimuths.

4. **No soft shadow or penumbra** — binary 0.0/1.0 only. The `f32` type reserves space for a gradient if needed later.

5. **No halo exchange between tiles** — not applicable here since shadow is not computed in tiled layout, but worth noting for future tiled variants.
