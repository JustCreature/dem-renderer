# Phase 3: Sun Shadow Sweep — Reference Document

---

## 1. The Shadow Problem

A pixel is in shadow if any terrain between it and the sun is tall enough to block the line of sight. Naively: march a ray from each pixel toward the sun, check each sample — O(N³), impractical for 3601².

**Sweep algorithm (O(N²))**: walk along rows (west→east for sun from the west). Maintain a running maximum of "effective height." One pass per row, one step per pixel → ~13M operations for 3601².

**Output**: `ShadowMask { data: Vec<f32>, rows, cols }` — `1.0` = lit, `0.0` = shadow. `f32` rather than `u8` allows soft shadow blending in Phase 4. Size: 3601² × 4 = **52 MB**.

---

## 2. The h_eff Formula

**Key formula** (plus sign is critical):
```
h_eff[c] = height[c] + c × dx × tan(θ)
```

**Derivation**: trace a backward ray from pixel `c` toward the sun. At column `c'` (west of `c`), the ray is at height `height[c] + (c - c') × dx × tan(θ)`. Pixel `c` is shadowed if `height[c'] > ray_height(c')`. Rearranging:
```
height[c'] + c' × dx × tan(θ)  >  height[c] + c × dx × tan(θ)
```
Both sides have the same form → define h_eff[c] = height[c] + c × dx × tan(θ). Pixel `c` is in shadow iff `max(h_eff[0..c-1]) > h_eff[c]`.

**Verification on flat terrain** (all heights = H): `h_eff[c]` strictly increases → running max is always less than current → no shadows ✓

**The bug with minus sign**: `height - dist × tan_sun` decreases monotonically on flat terrain → every pixel after col 0 appears in shadow → all-black PNG. Caught empirically.

**Update order**: check shadow BEFORE updating `running_max`. Initialize `running_max = f32::NEG_INFINITY` so column 0 is never shadowed.

---

## 3. Scalar Implementation

```rust
for r in 0..hm.rows {
    let mut running_max = f32::NEG_INFINITY;
    for c in 0..hm.cols {
        let h_eff = hm.data[r * hm.cols + c] as f32 + c as f32 * dx * tan_sun;
        if h_eff < running_max { data[r * hm.cols + c] = 0.0; }
        running_max = running_max.max(h_eff);
    }
}
```

**Bandwidth formula**: `2 × rows × cols` (read i16) + `4 × rows × cols` (write f32) = **6 bytes/pixel** → 78 MB total.

---

## 4. Branchless vs Branchy

```rust
// Branchless:
let in_shadow = (h_eff < running_max) as u32 as f32;
data[r * hm.cols + c] = 1.0 - in_shadow;
```

**Branchy: 8.1 GB/s | Branchless: 10.6 GB/s** (31% faster).

Shadow boundaries are spatially coherent → TAGE branch predictor achieves >99% accuracy → few mispredictions. Branchless wins anyway because:
1. **Regular store pattern** — unconditional sequential stores are more pipeline-friendly than conditional ones
2. **Branch uops consume ROB slots and frontend bandwidth** even when correctly predicted

**Lesson**: "avoid branches to prevent misprediction" is oversimplified. Branchless sometimes wins because it produces more regular, pipelineable instruction patterns.

---

## 5. NEON Vectorization

**Why vectorize across rows, not along columns**: each pixel depends on the previous (loop-carried `running_max`) — no intra-row parallelism. Rows are fully independent — process 4 rows simultaneously, each with its own `running_max` lane.

**Key NEON intrinsics**:

| Intrinsic | Operation |
|---|---|
| `vdupq_n_f32(x)` | broadcast scalar to all 4 lanes |
| `vld1q_f32(ptr)` | load 4 consecutive f32 |
| `vcltq_f32(a, b)` | 4-wide compare less-than → uint32x4_t mask |
| `vbslq_f32(mask, a, b)` | select: a where mask=1, b where mask=0 |
| `vmaxq_f32(a, b)` | 4-wide running max |
| `vgetq_lane_f32::<N>(v)` | extract lane N as scalar |

**`vbslq_f32` argument order**: `vbslq_f32(mask, on_true, on_false)`. Mask=1 when in shadow → `vbslq_f32(mask, 0.0, 1.0)`.

**Gather loads**: 4 heights at column `c` are at addresses `base[0]+c .. base[3]+c`, separated by ~7.2 KB. Not contiguous → scalar-load each into `[f32; 4]` array, then `vld1q_f32`. The M4 prefetcher can track ~8 streams simultaneously — 4 row streams train it well.

**Scatter stores**: cannot use `vst1q_f32` — outputs are ~14 KB apart. Use 4 × `vgetq_lane_f32:<N>` + scalar writes.

**Inner loop**:
```rust
while r + 4 <= hm.rows {
    let base = [r*cols, (r+1)*cols, (r+2)*cols, (r+3)*cols];
    let mut running_max = vdupq_n_f32(f32::NEG_INFINITY);
    for c in 0..cols {
        let heights = [hm.data[base[0]+c] as f32, /* +1,+2,+3 */];
        let h_vec = vld1q_f32(heights.as_ptr());
        let h_eff = vaddq_f32(h_vec, vdupq_n_f32(c as f32 * step));
        let mask = vcltq_f32(h_eff, running_max);
        let result = vbslq_f32(mask, vdupq_n_f32(0.0), vdupq_n_f32(1.0));
        *data.get_unchecked_mut(base[0]+c) = vgetq_lane_f32::<0>(result);
        // lanes 1–3
        running_max = vmaxq_f32(running_max, h_eff);
    }
    r += 4;
}
// scalar tail for remaining rows
```

---

## 6. Rayon Parallelism

**`par_chunks_mut` structure** (west-only sweep):
```rust
data.par_chunks_mut(4 * hm.cols).enumerate().for_each(|(chunk_idx, chunk)| {
    let r = chunk_idx * 4;
    // chunk is &mut [f32] — direct disjoint view into data
    // write: chunk[local_r * cols + c]
    // read:  hm.data[global_r * cols + c]  (shared immutable)
});
```

Rayon proves at compile time that chunks are non-overlapping. `&Heightmap` is `Sync` (read-only). No locks needed.

**For arbitrary azimuth**: uses `par_chunks` on `starting_pixels` + `SendPtr` for shared mutable output. See Section 9.

---

## 7. Performance Results

All numbers: M4 Max, cold cache, isolated runs.

| Implementation | Time | Bandwidth | vs Scalar |
|---|---|---|---|
| Scalar (branchy) | 10.2ms | 8.1 GB/s | 1× |
| Scalar (branchless) | 9.4ms | 10.6 GB/s | 1.3× |
| NEON 4-wide | 4.7ms | 17.4 GB/s | 2.2× |
| NEON parallel (10 cores) | 1.4ms | 58.6 GB/s | 7.3× |

**Optimization arc**:
```
Scalar:          latency-bound 64% (6.5ms) + memory 36% (3.7ms)  = 10.2ms
NEON 4-wide:     latency reduced 4×  → 1.6ms; memory ≈ 3.2ms     =  4.7ms
NEON parallel:   latency ≈ 0.16ms (negligible); memory ceiling    =  1.4ms
```

**Latency-bound analysis for scalar**: `3601² × 2 cycles (fmax latency) / 4 GHz ≈ 6.5ms`. Each `running_max = running_max.max(h_eff)` depends on the previous result — strict serial chain.

**NEON gives 2.2× not 4×**: NEON reduces latency component 4× (6.5 → 1.6ms) but memory is also a bottleneck (3.7ms). Combined improvement: `10.2ms → 4.8ms` ≈ 2.1×. Bottleneck-shift pattern — fixing the dominant bottleneck reveals the next one.

**Parallel gives 3.4× from parallelism, not 10×**: memory bandwidth is now the ceiling. 10 threads × 4 streams = 40 concurrent strided streams saturate the memory controller at ~58.6 GB/s.

---

## 8. DDA — Arbitrary Sun Azimuth

**DDA (Digital Differential Analyzer)**: grid traversal that visits every pixel a line passes through, in order, with no pixel visited twice. Normalize the step vector so the dominant axis advances ±1 per step.

**Sun direction** (azimuth α, clockwise from north, in grid coordinates where row increases southward):
```
dc = -sin(α)   (column: east = +)
dr =  cos(α)   (row: south = +)
```

Verification: α=270° (west) → `dc=1, dr=0` → purely eastward ✓

**DDA normalization**:
```
if |dc| >= |dr|:       dc_step = sign(dc); dr_step = dr / |dc|
else:                  dr_step = sign(dr); dc_step = dc / |dr|
```

**Entry edges** — rays enter from the opposite side of the sun:
```
dc_step > 0 → west edge  (c=0)
dc_step < 0 → east edge  (c=cols-1)
dr_step > 0 → north edge (r=0)
dr_step < 0 → south edge (r=rows-1)
```
For diagonal sun: both a row-edge and a column-edge apply → ~7200 starting pixels (vs ~3601 for cardinal). Corner overlap is harmless (processed twice, same result).

**Distance per step**:
```rust
let dist_per_step = ((dc_step * dx).powi(2) + (dr_step * dy).powi(2)).sqrt();
```
Needed because pixels aren't square at 47°N: `dy ≈ 30.9 m`, `dx ≈ 21.1 m`. West cardinal: `dist_per_step = 21.1 m`. 45° diagonal: `≈ 37.5 m`.

**DDA ray loop**:
```rust
for (start_r, start_c) in starting_pixels {
    let (mut r_f, mut c_f) = (start_r, start_c);
    let mut running_max = f32::NEG_INFINITY;
    let mut dist = 0.0f32;
    while in_bounds(r_f, c_f) {
        let h_eff = hm.data[r_f.round() as usize * cols + c_f.round() as usize] as f32
                  + dist * tan_sun;
        if h_eff < running_max { data[...] = 0.0; }
        running_max = running_max.max(h_eff);
        r_f += dr_step; c_f += dc_step; dist += dist_per_step;
    }
}
```

Use `round()` (not `floor()`) — keeps the path centered on the theoretical line, not biased to one side.

---

## 9. `dda_setup` Helper and NEON Parallel with Azimuth

**`DdaSetup` struct**: shared between scalar and NEON-parallel implementations.
```rust
struct DdaSetup {
    dc_step: f32, dr_step: f32,
    dist_per_step: f32,
    starting_pixels: Vec<(f32, f32)>,
}
```

**`compute_shadow_neon_parallel_with_azimuth` structure**:
```rust
starting_pixels.par_chunks(4).for_each(|rays| {
    // NEON loop while ALL 4 rays are in bounds
    while all_in_bounds { /* gather, h_eff, vmaxq, scatter */ }
    // Extract per-lane running_max with vgetq_lane_f32::<N>
    // Scalar continuation per surviving ray
});
```

**`SendPtr` required**: diagonal rays from two entry edges can visit the same corner pixel. Both writes produce valid `f32` (0.0 or 1.0). On AArch64, a 32-bit store is a single atomic instruction — no torn writes. The race is benign for visualization output.

Unlike the row-major version, DDA lanes track independent `(r_f, c_f)` positions — each lane computes its own pixel index. The `h_eff` formula and `vmaxq_f32` update are identical.

---

## 10. Solar Position Reference

Sun at 47°N latitude:

| Date | Azimuth at sunrise |
|---|---|
| Equinox (spring/autumn) | 90° (due east) |
| Summer solstice | ~50–60° (northeast) |
| Winter solstice | ~120–130° (southeast) |

Good test configurations:
- **Equinox sunrise**: azimuth=90°, elevation=10° — long shadows to the west
- **Winter solstice noon**: azimuth=180°, elevation=19.5° — shadows due north
- **Low dramatic sun**: any azimuth, elevation=5–10° — very long shadows

---

## 11. Data Sizes (N47, 3601×3601)

| Data | Size |
|---|---|
| Heightmap `Vec<i16>` | 26 MB |
| ShadowMask `Vec<f32>` | 52 MB |
| Total data movement per run | 78 MB |
| NEON parallel latency component | 0.16ms (negligible) |
| Memory bandwidth ceiling (10 cores) | ~58.6 GB/s |

---

## 12. Key Bugs Caught in Phase 3

| Bug | Symptom | Fix |
|---|---|---|
| Wrong h_eff sign (`-` instead of `+`) | All-black PNG | Geometric re-derivation via backward ray |
| `vbslq_f32` args swapped | Lit/shadow inverted | Understand `(mask, on_true, on_false)` order |
| `vst1q_f32` for scatter stores | Wrong pixels written | Use 4 × `vgetq_lane_f32` + scalar stores |
| `hm.data[r*cols + r]` copy-paste typo | Silent wrong output | Read index uses `r` for `c` |
| Wrong bandwidth formula (copied normals) | Inflated GB/s numbers | 6 bytes/pixel for shadow, not 20 |
| Nested for-c in starting_pixels loop | 3601× too many starting pixels | Collect starting pixels in loop over edges only |

---

## 13. Open Items

- `compute_shadow_neon_parallel_with_azimuth` compilation not yet confirmed by user — awaiting test run
- Phase 1 `profiling::timed` label bug: `random_read`/`seq_write`/`random_write` all report label `"seq_read"`
- `fill_nodata` division-by-zero if all 4 directions hit boundary without valid data
- Tiled normal computation leaves cross-tile boundary rows as zero — requires halo exchange to fix
