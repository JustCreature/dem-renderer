# Phase 6 — Experiment Matrix: Comprehensive Student Textbook

**Hardware**: Apple M4 Max (10 performance cores, 4 efficiency cores, 400 GB/s unified memory bandwidth, 128 KB L1D per perf core, 16 MB L2, 48+ MB SLC)
**Dataset**: USGS SRTM N47E011, 3601×3601, ~26 MB as i16 / ~52 MB as f32
**All benchmarks cold cache unless noted. Date: 2026-04-06**

---

## Overview

Phase 6 is an experiment matrix: nine isolated micro-benchmarks, each varying one variable while holding everything else fixed. The goal is not to build new features but to develop the intuition to predict performance bottlenecks before measuring — and to correct those predictions with real numbers.

The experiments are grouped thematically:
- **Exps 1–3**: layout and parallelism on a synthetic stencil kernel
- **Exps 4–5**: data layout for a multi-component structure (AoS vs SoA, Morton vs row-major)
- **Exps 6–7**: instruction-level and memory-level parallelism (prefetch, NEON accumulators)
- **Exps 8–9**: memory access geometry (gather cost, TLB working set)

---

## Part 1: The Stencil Kernel and Vectorisation

### 1.1 Why a Synthetic Kernel?

The terrain renderer's hot paths (normals, shadows, raymarching) mix several variables: SIMD width, memory layout, parallelism, branching. To isolate the effect of each, Phase 6 uses a **5-point stencil sum**:

```
output[r][c] = data[r][c] + data[r-1][c] + data[r+1][c] + data[r][c-1] + data[r][c+1]
```

- Input: i16 heightmap (row-major, 3601×3601)
- Output: f32 row-major buffer
- **Logical bytes per pixel**: 5 × 2 (reads) + 1 × 4 (write) = 14 bytes
- **Total per run**: (3601−2)² × 14 ≈ 181 MB
- No shadow dependency, no ray marching, no SIMD complexity to confuse the measurement

### 1.2 Auto-Vectorisation and the `continue` Branch

The row-major version iterates straightforwardly:

```rust
for r in 1..rows-1 {
    for c in 1..cols-1 {
        output[r*cols+c] = data[r*cols+c] as f32 + ...;
    }
}
```

No branches in the inner loop → LLVM auto-vectorises to **8-wide NEON** (`ld q0`, `fadd.4s` × 2, `stp q1/q0`, loop stride = 8 pixels), confirmed by inspecting the emitted assembly.

The tiled version had a border check inside the inner loop:

```rust
if global_r == 0 || global_r >= rows-1 || global_c == 0 || global_c >= cols-1 {
    continue;
}
```

LLVM cannot vectorise a loop that contains a `continue` targeting the loop itself. Even though the branch is taken only ~0.03% of the time (border pixels), the **presence** of the branch — not its frequency — is what prevents vectorisation. The resulting code uses scalar halfword loads (`ldr h0`) and scalar float adds (`fadd s0`).

**Result (Exp 1, cold cache, 3601×3601):**

| Variant | GB/s |
|---|---|
| row-major (auto-vec 8-wide NEON) | 60–72 GB/s |
| tiled T=32 (scalar) | 10.13 GB/s |
| tiled T=64 (scalar) | 10.76 GB/s |
| tiled T=128 (scalar) | 11.06 GB/s |
| tiled T=256 (scalar) | 11.01 GB/s |

The ~6× gap equals approximately the NEON vectorisation width (8×) minus prefetcher benefits for the tiled variant. All tile sizes are identical — tile size is irrelevant when the bottleneck is the scalar loop itself.

**The fix**: a 9-region split — compute the valid range before the loop, then iterate [r_interior_start..r_interior_end] × [c_interior_start..c_interior_end] with no branches in the hot path. The ~6% border pixels are handled in a separate scalar pass. This was not implemented because the learning objective (prove the `continue` is the sole cause) was satisfied by assembly inspection.

### 1.3 Lesson: Compiler Visibility, Not Access Frequency

The standard mental model says "branch prediction makes rare branches free." That is true for **execution cost** but false for **vectorisation**. The auto-vectoriser requires a static guarantee that the loop body executes unconditionally for every iteration. A `continue` — regardless of how rarely it fires — breaks that guarantee. The solution is to push conditional logic outside the hot loop, not to make it less frequent.

---

## Part 2: Thread Scaling and the Read/Write Asymmetry

### 2.1 Parallelising the Stencil

The row-major stencil was parallelised with Rayon's `par_chunks_mut`, giving each thread a contiguous slice of output rows. Cache coherence: each thread writes to its own rows (no false sharing between rows, since rows are ≥7202 bytes = 112 cache lines apart).

**Unexpected gotcha**: the single-threaded Rayon closure ran at **11 GB/s**, matching the tiled scalar kernel, not the 60–72 GB/s row-major baseline. Root cause: same as the tiled case — the closure boundary prevents auto-vectorisation. The compiler can't inline across a `FnMut` trait boundary, so it can't vectorise the inner loop.

This means Exp 2's "1-thread baseline" is 11 GB/s (scalar in closure), not 60 GB/s (scalar function). The speedup numbers are relative to this scalar baseline.

**Exp 2 Results (with output write):**

| Threads | GB/s | Speedup vs 1T |
|---|---|---|
| 1 | 11.2 | — |
| 2 | 22.7 | 2.0× |
| 4 | 37.9–43.3 | 3.4–3.9× |
| 8 | 83.9–85.2 | 7.5–7.6× |
| 10 | 85.8–96.6 | 7.7–8.6× |
| 12 | 100.9–103.4 | 9.0–9.2× |

Linear scaling to 8 threads, then flattening. The hypothesis: write-path pressure is the ceiling.

### 2.2 The Write Path is Narrower Than the Read Path

To test the hypothesis, Exp 3 ran the same stencil but **accumulated into a per-thread i64 scalar** instead of writing to an output buffer. No output array: only reads.

**Exp 3 Results (read-only):**

| Threads | Read-only GB/s | Write GB/s | Ratio |
|---|---|---|---|
| 1 | 31.9–34.8 | 11.2 | ~3× |
| 2 | 64.3 | 22.7 | 2.8× |
| 4 | 120.0 | 37.9 | 3.2× |
| 8 | 226.3 | 85.2 | 2.7× |
| 10 | 259.0 | 96.6 | 2.7× |
| 12 | 246.9 | 100.9 | 2.4× |

Three findings:

1. **~3× throughput difference at all thread counts** — removing output writes triples throughput. The write path (store buffer → L1D writeback → L3 → DRAM) is ~3× narrower than the read path.

2. **Scaling continues linearly past 8 threads in read-only** — confirming that the write path was causing the Exp 2 plateau at 8 threads, not DRAM read bandwidth.

3. **Ceiling at ~259 GB/s at 10 threads** — this is the true sustained DRAM **read** bandwidth, ~65% of the 400 GB/s theoretical peak. Achieving peak requires perfectly streaming sequential accesses; this ~35% efficiency gap is normal and represents the overhead of ECC, refresh cycles, memory controller scheduling, and coherence traffic.

### 2.3 Why the Write Path is Narrower

The M4 Max's memory subsystem is asymmetric by design:

- **Reads**: hardware prefetch units train on sequential access patterns and issue cache-line fills proactively. Up to 20+ fill buffers outstanding per core. Sequential reads can pipeline to saturate DRAM.
- **Writes**: store buffer (finite, ~72 entries on M4) must drain to L1D, which must writeback to L2, L3, then DRAM via the write-combining queue. Write-combining merges stores to the same cache line, but the physical write bandwidth through the writeback path is limited. Writes cannot be "prefetched"; they stall when the store buffer fills.

The 3× asymmetry is a real hardware property, not a software artifact. For write-heavy kernels (anything that produces an output buffer), bandwidth saturation occurs at fewer cores.

---

## Part 3: AoS vs SoA — When Layout Matters and When It Doesn't

### 3.1 Definitions

**AoS (Array of Structures)**: one array of records. For a 3-component normal: `Vec<[f32; 3]>`, 12 bytes per pixel, nx/ny/nz interleaved.

**SoA (Structure of Arrays)**: three separate arrays. The current `NormalMap`: `nx: Vec<f32>`, `ny: Vec<f32>`, `nz: Vec<f32>`, each 4 bytes per pixel, contiguous per component.

### 3.2 When SoA Should Win

For a kernel that reads **only one component** (e.g., nz for horizon checking), SoA reads 4 bytes/pixel while AoS must load 12 bytes/pixel (the entire record) to reach nz. AoS wastes 2/3 of every cache line. With a large enough array and enough cores, this 3× bandwidth waste drives a 3× throughput difference.

### 3.3 What We Actually Measured

**Exp 4 Results (M4 Max, 12 Mpix, 155 MB AoS / 3×51 MB SoA):**

| Kernel | SoA | AoS | Ratio |
|---|---|---|---|
| dot(normal, sun) — 12 B/pix | 22.4 GB/s | 22.4 GB/s | 1.00× |
| nz-only — 4 B/pix logical | 8.0 GB/s | 8.1 GB/s | 1.00× |
| nz-only — 10 threads | 66.0 GB/s | 58.3 GB/s logical (175 GB/s actual) | 1.1× |

**Dot product (same for both)**: Both layouts move 155 MB total. Sequential access, same total bytes, same prefetch behaviour — same time, same throughput. Expected.

**nz-only single thread (unexpectedly tied)**: The bottleneck was not memory bandwidth but the **serial accumulation dependency chain**. `sum += nz[i]` generates `fadd s0, s0, s1` — each instruction depends on the previous result (3-cycle NEON fadd latency). The CPU can issue at most 1 fadd per 3 cycles regardless of how fast data arrives. At ~8 GB/s and 4 bytes/pixel: 51 MB / 8 GB/s = 6.4 ms. The AoS loop, despite loading 12 bytes/pixel from DRAM, takes the same 6.4 ms because the compute is the bottleneck, not memory.

**nz-only 10 threads (SoA wins 1.1×)**: Rayon gives each thread its own accumulator, breaking the serial dependency. Now memory is the bottleneck. SoA reads 51 MB at 66 GB/s actual; AoS reads 155 MB at 175 GB/s actual. AoS needs 175/259 = 68% of the DRAM ceiling; SoA needs only 25%. The times differ by 1.1× because the M4 Max's 400 GB/s bus has enough headroom to serve AoS at 175 GB/s without saturation. On a bandwidth-saturated machine (e.g., desktop CPU at 50 GB/s), the same experiment would show ~3×.

**The lesson**: SoA's bandwidth advantage only manifests when (a) the access is genuinely memory-bound (not compute-bound from a serial reduction chain), and (b) the memory subsystem is near saturation. On the M4 Max's wide bus, both conditions are rarely met simultaneously at single-thread scale.

---

## Part 4: Morton vs Row-Major Tile Ordering

### 4.1 The Z-Order (Morton) Curve

A **Morton curve** (Z-order curve) interleaves the bits of 2D coordinates to produce a 1D index that preserves 2D locality:

```
morton2(x, y) = spread_bits(x) | (spread_bits(y) << 1)
```

where `spread_bits` interleaves a 16-bit integer's bits with zeros: `0101 0101 0101 0101`. Tiles at (tr, tc) and (tr±1, tc) are now close in the flat index — typically 1–4 positions apart — rather than `tile_cols` positions apart in row-major.

**Why this should matter for the stencil**: the 5-point stencil accesses north and south neighbours in adjacent tile rows. In row-major tile order, the north tile is `tile_cols = 57` tiles before the current tile, i.e., 57 × 64² × 2 = 468 KB away in memory. The L1D is 128 KB — this is always an L1 miss, though it likely hits L2 (16 MB on M4 Max). With Morton order, the north tile is 1–4 tiles away = 8–32 KB, fitting in L1D.

### 4.2 What We Measured

**Exp 5 Results (T=64, cold cache):**

| Variant | GB/s |
|---|---|
| Row-major (vectorised baseline) | 68.1 GB/s |
| Tiled row-major T=64 | 10.40 GB/s |
| Tiled Morton T=64 | **10.43 GB/s** (1.00×) |

Morton is within noise of row-major tiled. The N/S cache distance improvement (456 KB → 8–32 KB) had zero measurable effect.

**Why**: the scalar loop is the bottleneck. Each pixel takes ~4–5 cycles of compute (load, convert i16→f32, fadd, fadd, fadd, fadd, store). At 3.7 GHz, that's ~1.2 ns/pixel. For an L2 cache hit (468 KB distance, ~12 cycle latency), the processor must issue the north-tile load well before it's needed. With an OOO ROB of ~600 entries and a loop body of ~10 instructions, the processor can "see ahead" ~60 iterations — more than enough to absorb L2 latency. The L2 miss is hidden inside the computation.

**The unified lesson of Experiments 1 and 5**: the `continue` branch preventing vectorisation is so dominant that neither tile size nor tile ordering has any effect. The scalar arithmetic is the floor. Two different layout optimisations, each promising on paper, both produce 1.00× improvement over baseline tiled.

---

## Part 5: Software Prefetch — When the Hardware Already Wins

### 5.1 What Software Prefetch Does

`prfm pldl1keep, [addr]` (ARM) / `prefetcht0 [addr]` (x86): issues a non-blocking demand request to bring the cache line at `addr` into L1. The instruction returns immediately; the CPU begins the fill in the background. If issued far enough ahead, the data arrives before the dependent load instruction executes — latency hidden.

The expected model for random reads:
- Without prefetch: each load stalls for ~80 ns DRAM latency (sequential dependency chain: `sum += data[indices[i]]` means next load depends on finishing the current one)
- With prefetch D ahead: D cache misses in flight simultaneously → throughput = D × 64 bytes / 80 ns
- M4 Max fill buffer depth: ~20 → ceiling ≈ 20 × 64 / 80e-9 = 16 GB/s

### 5.2 What We Measured

**Exp 6 Results (64M floats = 256 MB, shuffled indices):**

| Variant | GB/s | vs baseline |
|---|---|---|
| No prefetch | 1.20 GB/s | — |
| prfm D=4 | 1.28 GB/s | +7% |
| prfm D=16 | 1.31 GB/s | +9% |
| prfm D=64 | 1.37 GB/s | +14% |

Only a 14% improvement at D=64, far below the expected ~13× from the latency-hiding model.

**Why the hardware already wins**: the M4 Max has a ~600-instruction ROB. For a simple scalar loop `sum += data[indices[i]]`, the OOO engine can look 64+ iterations ahead, see the load at `indices[i+64]`, and issue it speculatively. This is **hardware memory-level parallelism via OOO execution** — the same effect as software prefetch, but automatic. The explicit `prfm` only marginally adds to what the hardware is already doing.

**When software prefetch does help**: on an in-order processor (early ARM Cortex-A5, RISC-V embedded, some DSPs), or when the access pattern is too irregular for the OOO engine to predict (e.g., pointer-chasing linked lists where the next address isn't visible until the current load completes). On a superscalar OOO machine with a large ROB, software prefetch for simple loop patterns is largely redundant.

**Note on baseline vs Phase 0**: the Phase 0 `random_read` measured 0.6 GB/s because the shuffle was inside the timed block. Here indices are pre-generated, measuring pure reads at 1.2 GB/s — consistent after accounting for the measurement difference.

---

## Part 6: NEON Multiple Accumulators — Breaking the Serial Dependency Chain

### 6.1 The Dependency Chain Problem

A reduction loop `sum += f(i)` generates a chain of instructions where each result feeds the next:

```
vfmaq_f32 v0, v1, v2   // v0 = v0 + v1*v2 (latency: 4–5 cycles on M4)
// must wait 4–5 cycles before issuing next vfmaq that uses v0
vfmaq_f32 v0, v3, v4   // depends on previous v0 — stalls
```

Even with NEON (128-bit, 4 f32 per instruction), if there's only one accumulator register, throughput is 1 instruction per 4–5 cycles — the pipeline is not full.

### 6.2 Multiple Accumulators

With K independent accumulator registers, the scheduler can dispatch up to K instructions simultaneously on different FP/SIMD ports:

```
// 4 accumulators: a0, a1, a2, a3 — completely independent
a0 = vfmaq_f32(a0, x0, sx + ny0*sy + nz0*sz)  // issued cycle 1
a1 = vfmaq_f32(a1, x1, sx + ny1*sy + nz1*sz)  // issued cycle 1, different port
a2 = vfmaq_f32(a2, ...)                        // issued cycle 2
a3 = vfmaq_f32(a3, ...)                        // issued cycle 2
// a0 is ready after 4–5 cycles — by then we've issued 4 more FMAs
```

The M4 has 2 FP/SIMD execution ports per core. With 4+ accumulators, both ports can be kept busy every cycle — throughput becomes limited by memory bandwidth, not instruction latency.

### 6.3 Results

**Exp 7 Results (SoA dot product, 12 Mpix, 155 MB):**

| Variant | GB/s | Notes |
|---|---|---|
| Auto-vec scalar (implicit 1 acc) | 23.7 GB/s | LLVM generates NEON but 1 acc |
| NEON explicit 1 acc | 18.0 GB/s | **Slower** — FMA latency 4–5 vs scalar 3 cycles |
| NEON explicit 4 acc | **71.0 GB/s** | 3.0× speedup over auto-vec |
| NEON explicit 8 acc | **120.0 GB/s** | L3/SLC bandwidth ceiling |

**Surprising result: NEON 1 acc is SLOWER than auto-vec scalar.** The auto-vectorised scalar loop uses scalar `fadd` with 3-cycle latency; explicit NEON `vfmaq_f32` has 4–5 cycle latency. When there's only one accumulator, higher instruction latency = lower throughput regardless of lane width. The compiler also schedules the scalar version better.

**4 accumulators: 23.7 → 71 GB/s (3×)**. This crosses the transition from compute-bound to memory-bound. The 68 GB/s "ceiling" from Exp 1 is exceeded slightly because the stencil had i16→f32 conversion overhead; pure f32 sequential SoA reads run slightly faster.

**8 accumulators: 120 GB/s.** Exceeds DRAM bandwidth. The eviction buffer is 100 MB but the three SoA arrays total 153 MB — the evict does not fully flush all data. Remaining data hits the M4 Max's SLC (System Level Cache, estimated 48+ MB with 200+ GB/s bandwidth). 8 accumulators provide enough ILP to saturate the SLC pipeline.

**The transition model:**

```
1 accumulator  → compute-bound  (fadd latency 3–5 cycles per pixel)
4 accumulators → DRAM-bound     (~65 GB/s, all ports busy, memory is the limit)
8 accumulators → SLC-bound      (~120 GB/s, data partially in SLC)
```

Each represents a different hardware bottleneck. Adding more accumulators past the memory-bound transition has no effect on a cold-cache run where all data comes from DRAM.

---

## Part 7: Ray Packet Gather Cost

### 7.1 The Phase 4 Mystery

Phase 4 showed: NEON 4-ray packet single-thread = scalar single-thread = 0.80s. The hypothesis was that gather overhead cancelled the 4× SIMD gain. Phase 6 Exp 8 measures this directly.

### 7.2 Cache Line Geometry

The heightmap is 3601 columns × 2 bytes = 7202 bytes per row. A cache line is 64 bytes = 32 i16 values. Therefore:

- **Same-row neighbours** (column ±1): offset 2 bytes, same cache line → 0 extra misses
- **Adjacent columns** (c, c+1, c+2, c+3): 8 bytes total, same cache line → 1 miss for 4 values
- **Adjacent rows** (r, r+1): offset 7202 bytes → always different cache line → 2 misses
- **Strided rows** (r, r+1, r+2, r+3): 4 separate cache lines → 4 misses

### 7.3 Results (per step = one memory access group)

**Exp 8 Results (2M random positions in 3601×3601 heightmap):**

| Variant | Total time | Steps | ns/step | Pixels/step | ns/pixel |
|---|---|---|---|---|---|
| 1-wide scalar | 3.4 ms | 2M | 1.7 ns | 1 | 1.70 ns |
| 4-wide adjacent cols | 4.6 ms | 2M | 2.3 ns | 4 | **0.57 ns** |
| 4-wide strided rows | 6.1 ms | 2M | 3.1 ns | 4 | **0.76 ns** |
| 4-wide fully random | 2.1 ms | 512K | 4.1 ns | 4 | **1.03 ns** |

**Adjacent columns (best case)**: 4 values in one cache line = ~free. 2.3 ns/step for 4 pixels vs 1.7 ns/step for 1 pixel → **3× pixels per unit time**. NEON packets at the start of a ray (adjacent screen pixels, same DEM row) genuinely help.

**Strided rows (diverged case)**: 4 separate cache misses. The OOO engine issues all 4 in parallel, so time is only 1.8× the 1-wide case despite 4× the misses → still **2.2× pixels per unit time**.

**Fully random (maximum divergence)**: 4 truly independent positions. Grouping them in one step allows the CPU to issue 4 loads in parallel — better memory-level parallelism than 1-wide's serial reduction chain → **1.7× pixels per unit time**.

### 7.4 Why Phase 4 NEON = Scalar Despite These Numbers

Memory access alone suggests NEON packets always win (1.7–3× pixels/ns). The Phase 4 result of exactly 1.00× suggests the bottleneck was elsewhere:

1. **Ray height computation is the bottleneck, not loads.** Each step computes `ray_z = origin_z + t * dir_z`, compares against `terrain_z`, and conditionally advances. This arithmetic doesn't vectorise cleanly in a packet because the 4 rays diverge: they travel different distances and may terminate at different steps.

2. **Divergence kills the packet.** At step 0 all 4 pixels are adjacent. After ~10–50 steps, the 4 rays (same column, adjacent rows) have marched to different terrain heights and different (row, col) positions. The NEON packet now performs effectively-random gathers with no spatial correlation.

3. **The compiler auto-vectorises scalar.** The scalar render loop is simple enough that LLVM generates NEON from scalar code. The "NEON" benefit was already included in the "scalar" baseline.

---

## Part 8: TLB Working Set Sweep

### 8.1 The TLB and Page Tables

The **Translation Lookaside Buffer (TLB)** caches virtual-to-physical address translations. Every memory access first checks the TLB:

- **TLB hit**: address translated in ~1 cycle. The physical address goes to the cache hierarchy.
- **TLB miss**: the hardware page walker traverses the page table tree (4 levels on AArch64 with 4-level translation, 2–4 on Apple Silicon). Each level is a memory access. Total: 50–100 cycles.

**Apple Silicon page sizes**: unlike x86 Linux (4 KB), macOS/M-series uses **16 KB base pages**. This expands TLB coverage by 4×:

| Metric | x86 (4KB pages) | M4 Max (16KB pages) |
|---|---|---|
| L1 DTLB entries | ~64 | ~256 |
| L1 DTLB coverage | ~256 KB | **~4 MB** |
| L2 TLB entries (est.) | ~1500 | ~3000 |
| L2 TLB coverage | ~6 MB | **~48 MB** |

### 8.2 Results

**Exp 9 Results (1M random reads, 4 bytes each, macOS/M4 Max):**

| Working set | 16KB pages | GB/s | Layer |
|---|---|---|---|
| 16 KB | 1 | 8.20 GB/s | L1 cache + L1 DTLB |
| 256 KB | 16 | 8.20 GB/s | L2 cache + L1 DTLB |
| 1 MB | 64 | 7.51 GB/s | L3/SLC + L1 DTLB |
| **4 MB** | **256** | **6.40 GB/s** | ← L1 DTLB filling (256 entries × 16KB) |
| **16 MB** | **1024** | **3.77 GB/s** | ← L2 TLB pressure begins |
| 64 MB | 4096 | 1.91 GB/s | L2 TLB miss → page walks |
| 256 MB | 16384 | 1.38 GB/s | page walks + DRAM latency |

**First knee at 4 MB (256 pages)**: 8.2 → 6.4 GB/s. Confirms L1 DTLB has ~256 entries × 16KB = 4 MB. Beyond this, each access pays the L2 TLB lookup cost (~5–10 extra cycles).

**Second knee at 16–64 MB**: 6.4 → 3.8 → 1.9 GB/s. L2 TLB exhausted at ~1000–2000 entries × 16KB = 16–32 MB. Beyond this, full page table walks add ~50 cycles per unique page.

### 8.3 Relevance to the Renderer

The 26 MB heightmap = 1625 × 16KB pages. L2 TLB capacity is ~48 MB, so all 1625 pages fit in L2 TLB — no page table walks. The ~1.31 GB/s measured in Exp 8 for random heightmap access is consistent with the 16–64 MB band (paying L2 DTLB costs and L3/SLC cache misses, but no page walks).

The 256 MB benchmark array (64M floats) = 16384 pages. This exceeds L2 TLB → page walks → the Exp 6 random-read baseline of 1.2 GB/s includes substantial page-walk overhead on top of DRAM latency.

**Huge pages**: on x86 Linux, 2 MB huge pages would reduce 256 MB to 128 entries → fits L1 DTLB. On macOS M4, the equivalent "large pages" are ~32 MB (not 2 MB) but are managed transparently by the OS — no userspace API. The 4× wider base page (16KB vs 4KB) already provides much of this benefit implicitly.

---

## Part 9: Common Errors and Wrong Hypotheses

### 9.1 "Tile size should affect the tiled stencil performance"

**Wrong hypothesis**: T=32 (2 KB/tile, fits L1) should be faster than T=256 (128 KB/tile, fills L1).
**Measurement**: all tile sizes within noise, 10–11 GB/s.
**Cause**: the bottleneck is the scalar loop (from the `continue` branch blocking vectorisation), not cache reuse. Tile size only matters when the inner loop can actually benefit from spatial locality, which requires the loop to be executing fast enough to be memory-bound.

### 9.2 "Morton ordering should help the stencil's N/S neighbour access"

**Wrong hypothesis**: Morton tiles reduce the N/S tile distance from 456 KB to 8–32 KB → L1 hits instead of L2 hits → measurable speedup.
**Measurement**: Morton = row-major tiled, 1.00×.
**Cause**: the scalar computation is the bottleneck, not cache latency. The OOO engine's ROB (~600 entries) can absorb L2 latency (~12 cycles) inside the compute time of ~60 instructions ahead. Morton reduces miss cost but the cost was already hidden.

### 9.3 "Removing output writes should give a small improvement"

**Wrong hypothesis**: writes are cheap because write-combining batches them; maybe 10–20% faster without writes.
**Measurement**: ~3× faster without writes at all thread counts.
**Cause**: the write path (store buffer → L1D writeback → L3 → DRAM) is genuinely ~3× narrower than the read path. Write-combining helps within a cache line but the writeback bandwidth is fundamentally limited by the DRAM write throughput, which is lower than DRAM read throughput on M4 Max.

### 9.4 "Software prefetch should dramatically improve random reads"

**Wrong hypothesis**: M4 Max random reads at 1.2 GB/s are serial-latency-bound; adding `prfm` D=64 should give 8–16 GB/s by keeping 64 misses in flight.
**Measurement**: +14% at D=64.
**Cause**: the M4 Max's ~600-instruction ROB already issues speculative loads far ahead for simple loop patterns. Explicit `prfm` is largely redundant. Software prefetch gives large gains on in-order or small-ROB processors, not on aggressive OOO superscalars.

### 9.5 "NEON explicit 1-accumulator should match auto-vectorised scalar"

**Wrong hypothesis**: explicit NEON gives at least the same performance as auto-vec (same instruction count, same data).
**Measurement**: explicit NEON 1-acc = 18.0 GB/s, worse than auto-vec 23.7 GB/s.
**Cause**: NEON `vfmaq_f32` latency is ~4–5 cycles; scalar `fadd` latency is ~3 cycles. With a serial dependency chain (1 accumulator), higher instruction latency = lower throughput. The auto-vectorised code also has better load scheduling from LLVM's instruction scheduler.

### 9.6 "AoS nz-only should be 3× slower than SoA nz-only due to 3× cache waste"

**Wrong hypothesis**: reading 51 MB (SoA) vs 155 MB (AoS) for the same useful data should produce 3× throughput difference.
**Measurement**: single-thread 1.00×; 10-thread 1.1×.
**Cause**: single-thread is compute-bound (serial fadd chain bottleneck), not memory-bound. The 3× bandwidth waste doesn't matter when the CPU isn't processing data fast enough to saturate memory. With 10 threads the bottleneck shifts to memory, but the M4 Max's 400 GB/s bus has enough headroom to serve AoS at 175 GB/s without saturation — the ratio compresses to 1.1× instead of 3×.

---

## Part 10: Full Benchmark Tables

**Hardware**: Apple M4 Max, 10 perf cores, 400 GB/s peak DRAM. **Date**: 2026-04-06.
**All results cold cache (100 MB eviction before each run) unless noted.**

### Experiment 1: Tile Size Sweep
Kernel: 5-point stencil, 3601×3601, 181 MB/run, cold cache.

| Variant | GB/s |
|---|---|
| row-major (auto-vec 8-wide NEON) | 60–72 |
| tiled T=32 | 10.13 |
| tiled T=64 | 10.25–10.76 |
| tiled T=128 | 10.67–11.06 |
| tiled T=256 | 10.84–11.01 |

### Experiment 2: Thread Scaling (with writes)
Kernel: row-major stencil via Rayon `par_chunks_mut`, cold cache.

| Threads | GB/s |
|---|---|
| 1 | 11.2–11.5 |
| 2 | 22.7–22.9 |
| 4 | 37.9–43.3 |
| 6 | 42.9–66.9 |
| 8 | 83.9–85.2 |
| 10 | 85.8–96.6 |
| 12 | 100.9–103.4 |

### Experiment 3: Thread Scaling (read-only)
Kernel: row-major stencil, per-thread i64 accumulator, no output write.

| Threads | GB/s |
|---|---|
| 1 | 31.9–34.8 |
| 2 | 64.3 |
| 4 | 120.0 |
| 8 | 226.3 |
| 10 | 259.0 |
| 12 | 246.9 |

### Experiment 4: AoS vs SoA
Normal map, 12 Mpix, 155 MB AoS / 51 MB per SoA component.

| Kernel | SoA | AoS | Ratio |
|---|---|---|---|
| dot product (12 B/pix) | 22.4–24.2 GB/s | 22.4–24.1 GB/s | 1.00× |
| nz-only, 1 thread | 8.0–8.3 GB/s | 8.1–8.4 GB/s | 1.00× |
| nz-only, 10 threads | 66.0 GB/s | 58.3 GB/s (175 GB/s actual) | 1.13× |

### Experiment 5: Morton vs Row-major Tile Ordering
5-point stencil, T=64, cold cache.

| Variant | GB/s |
|---|---|
| row-major (vectorised baseline) | 64–68 |
| tiled row-major T=64 | 10.25–10.40 |
| tiled Morton T=64 | 10.43 |

### Experiment 6: Software Prefetch
256 MB array, 64M random reads, pre-shuffled indices.

| Variant | GB/s |
|---|---|
| no prefetch | 1.20 |
| prfm D=4 | 1.28 |
| prfm D=16 | 1.31 |
| prfm D=64 | 1.37 |

### Experiment 7: NEON Multiple Accumulators
SoA dot product, 12 Mpix, 155 MB total.

| Variant | GB/s |
|---|---|
| auto-vec scalar | 23.7 |
| NEON explicit 1 acc | 18.0 |
| NEON explicit 4 acc | 71.0 |
| NEON explicit 8 acc | 120.0 |

### Experiment 8: Ray Packet Gather Cost
2M random positions in 3601×3601 heightmap (25 MB).

| Variant | GB/s | ns/pixel |
|---|---|---|
| 1-wide | 1.31 | 1.70 ns |
| 4-wide adjacent cols | 3.84 | 0.57 ns |
| 4-wide strided rows | 2.90 | 0.76 ns |
| 4-wide fully random | 8.35 (reported at 4× bytes) | 1.03 ns actual |

### Experiment 9: TLB Working Set Sweep
1M random reads (4 bytes each), macOS/M4 Max (16KB base pages).

| Size | 16KB pages | GB/s |
|---|---|---|
| 16 KB | 1 | 8.20 |
| 64 KB | 4 | 7.77 |
| 256 KB | 16 | 8.20 |
| 1 MB | 64 | 7.51 |
| 4 MB | 256 | 6.40 |
| 16 MB | 1024 | 3.77 |
| 64 MB | 4096 | 1.91 |
| 256 MB | 16384 | 1.38 |

---

## Summary

- **Vectorisation gates everything**: a single `continue` in the inner loop cut throughput 6× and made every other layout optimisation (tile size, Morton ordering) irrelevant. Control flow visibility is the most important property of a hot loop — more important than memory layout.

- **The write path is 3× narrower than the read path**: for write-heavy parallel kernels on M4 Max, bandwidth saturation occurs at ~8 cores. For read-only kernels it occurs at ~10 cores at ~259 GB/s. This asymmetry is a real hardware property of the store buffer → writeback → DRAM path.

- **Serial reduction chains are compute-bound, not memory-bound**: a plain `sum += data[i]` loop, even when logically memory-bound, is actually limited by the fadd latency chain. The memory bandwidth is only relevant once you break this chain (multiple accumulators, parallel reduction). NEON 1-acc is slower than scalar, not faster.

- **The M4 Max's OOO engine makes software prefetch largely redundant**: a ~600-instruction ROB automatically keeps multiple loads in flight for simple loop patterns. Explicit `prfm` adds only 14% on top. Software prefetch provides large gains only on in-order or small-ROB processors.

- **macOS 16KB pages give 4× TLB coverage vs x86**: L1 DTLB covers ~4 MB (not 1 MB); L2 TLB covers ~48 MB (not 12 MB). The 26 MB heightmap fits in L2 TLB, avoiding page table walks. The 256 MB benchmark array spills to page walks, adding ~50 cycles per unique page.

- **Ray packet memory gain (3×) is absorbed by compute divergence**: adjacent-column gathers share cache lines and deliver 3× pixels/ns, but rays diverge within ~50 steps, converting to strided/random access. The compute portion of each step (height comparison, ray advancement) doesn't vectorise as cleanly as the gather, netting 1.00× for the full render.

---

## Appendix: AVX2 Cross-Platform Port (added 2026-04-06)

After completing the 9 experiments on M4 Max (aarch64/NEON), all hot-path kernels were ported to AVX2 (x86_64, 256-bit, 8 × f32). This makes every benchmark reproducible on Intel/AMD hardware.

### What was ported

| NEON (aarch64) | AVX2 (x86_64) | Width | Notes |
|---|---|---|---|
| `compute_normals_neon` | `compute_normals_avx2` | 4→8 | i16→f32 via `_mm256_cvtepi16_epi32` + `_mm256_cvtepi32_ps` |
| `compute_normals_neon_tiled` | `compute_normals_avx2_tiled` | 4→8 | same tiled loop structure |
| `compute_shadow_neon` | `compute_shadow_avx2` | 8 rows | 8-wide row interleave, `_mm256_max_ps` running max |
| `compute_shadow_neon_parallel` | `compute_shadow_avx2_parallel` | 8 rows | rayon par + 8-wide inner |
| `compute_shadow_neon_parallel_with_azimuth` | `compute_shadow_avx2_parallel_with_azimuth` | 8 rays | reuses shared `dda_setup` from `shadow.rs` |
| `raymarch_neon` (4-wide) | `raymarch_avx2` (8-wide) | 4→8 | OOB masking via `_mm256_andnot_si256`, gather via store+index |
| `render_neon`, `render_neon_par` | `render_avx2`, `render_avx2_par` | 4→8 | `step_by(8)`, asserts `width % 8 == 0` |
| `seq_read_simd`, `random_read_simd` | `seq_read_avx2`, `random_read_avx2` | 4→8 | random uses `_mm256_i32gather_ps` (true hardware gather) |

All functions require `#[target_feature(enable = "avx2")]` and `unsafe fn`. Runtime dispatch uses `is_x86_feature_detected!("avx2")`.

### Key NEON → AVX2 intrinsic mapping

| Operation | NEON | AVX2 |
|---|---|---|
| broadcast | `vdupq_n_f32(x)` | `_mm256_set1_ps(x)` |
| load | `vld1q_f32(ptr)` | `_mm256_loadu_ps(ptr)` |
| store | `vst1q_f32(ptr, v)` | `_mm256_storeu_ps(ptr, v)` |
| add | `vaddq_f32` | `_mm256_add_ps` |
| multiply | `vmulq_f32` | `_mm256_mul_ps` |
| max | `vmaxq_f32` | `_mm256_max_ps` |
| compare LT | `vcltq_f32(a,b)` | `_mm256_cmp_ps::<1>(a, b)` |
| compare GE | `vcgeq_f32(a,b)` | `_mm256_cmp_ps::<13>(a, b)` |
| blend | `vbslq_f32(mask,t,f)` | `_mm256_blendv_ps(f, t, mask)` |
| and-not | `vbicq_u32(a, b)` = a&!b | `_mm256_andnot_si256(b, a)` (order reversed!) |
| all-zero test | `vmaxvq_u32(x) == 0` | `_mm256_testz_si256(x, x) != 0` |
| rsqrt | `vrsqrteq_f32` + `vrsqrtsq_f32` (NR) | `_mm256_rsqrt_ps` + manual NR step |
| gather | manual array + `vld1q_f32` | `_mm256_i32gather_ps(ptr, vindex, scale=4)` |
| i16→f32 | `vmovl_s16` + `vcvtq_f32_s32` | `_mm256_cvtepi16_epi32` + `_mm256_cvtepi32_ps` |
| lane extract | `vgetq_lane_f32::<N>` | `_mm256_storeu_ps(arr)` then `arr[N]` |

### Notable differences from NEON

**Register width**: AVX2 uses 256-bit registers (8 × f32) vs NEON's 128-bit (4 × f32). This doubles throughput for the same number of instructions, assuming the memory system can keep up.

**`_mm256_andnot_si256(b, a)` — argument order is reversed vs NEON**: `vbicq_u32(a, b)` computes `a & !b`. AVX2's `_mm256_andnot_si256(b, a)` also computes `a & !b`, but the argument labelled "b" is the one negated and it comes *first*. This is a common source of bugs when porting.

**`_mm256_i32gather_ps`** provides true hardware scatter-gather in one instruction. NEON has no equivalent — the NEON "random read" must build an array and call `vld1q_f32`. On x86_64, this enables cleaner random-access kernels, though gather throughput is lower than sequential load on current CPUs.

**`_mm256_rsqrt_ps` precision**: ~14-bit approximation (vs NEON's ~11-bit `vrsqrteq_f32`). One Newton-Raphson step (`y1 = y0 * (1.5 - 0.5 * x * y0²)`) brings both to ~23-bit (single-precision full).

**`unsafe fn` scope**: every AVX2 function requires `#[target_feature(enable = "avx2")]` and `unsafe fn`. Unlike NEON on aarch64 (always present), AVX2 is not universal on x86_64 — pre-Haswell CPUs lack it. The runtime `is_x86_feature_detected!("avx2")` guard is mandatory for safe dispatch.

### Platform dispatch pattern

Each public API (e.g. `compute_normals_vector`) dispatches at runtime:

```
aarch64 → NEON (always available, compile-time guaranteed)
x86_64  → AVX2 if is_x86_feature_detected!("avx2") [runtime]
x86_64  → scalar + eprintln!("[SCALAR FALLBACK] ...: AVX2 not detected") otherwise
other   → scalar + eprintln!("[SCALAR FALLBACK] ...: no SIMD for this architecture")
```

The `[SCALAR FALLBACK]` log lines make fallback paths visible in production logs. An x86_64 CPU running without AVX2 is unusual (AVX2 arrived in 2013 with Haswell) but the detection is required by Rust's safety model.

### The `render_vector` / `render_vector_par` special case

The AVX2 render path requires `width % 8 == 0` (packet width = 8 lanes). If the image width is not divisible by 8, the fallback message includes the actual width:

```
[SCALAR FALLBACK] render_vector: AVX2 not detected or width (1920) not divisible by 8
```

This reveals two distinct fallback reasons at a glance.

---

## Part 12: Cross-System Benchmark Synthesis

> Additional hardware tested: Windows Acer Nitro i5-9300H + GTX 1650 (DDR4, PCIe Gen3 ×4),
> Mac Intel i7-1068NG7 (32 GB LPDDR4X, Iris Plus iGPU), Asus Pentium N3700 (3 GB DDR3L, HD 405 iGPU).
> All four machines ran the same binary with `-C target-cpu=native`. Date: 2026-04-09.

### 12.1 System Profiles

| Property | M4 Max | Win Nitro | Mac i7 | Asus Pentium |
|---|---|---|---|---|
| ISA | AArch64 | x86-64 | x86-64 | x86-64 |
| Cores | 14 (10P+4E) | 4C/8T | 4C/8T | 4C/4T |
| DRAM | LPDDR5X unified | DDR4 | LPDDR4X | DDR3L |
| Peak BW | ~400 GB/s | ~43 GB/s | ~68 GB/s | ~20 GB/s |
| GPU | Built-in (shared mem) | GTX 1650 (PCIe ×4) | Iris Plus (shared) | HD 405 (shared) |
| OS page size | **16 KB** | 4 KB | 4 KB | 4 KB |
| L1 DTLB reach | **4 MB** | 256 KB | 256 KB | 256 KB |

### 12.2 Auto-Vectorization Penalty — Universal Across All ISAs

The `continue` branch in the tiled stencil inner loop cut throughput by 7–10× on every machine:

| Machine | Row-major (auto-vec) | Tiled (vec blocked) | Penalty |
|---|---|---|---|
| M4 Max | 60–72 GB/s | ~11 GB/s | **6.5×** |
| Win Nitro | ~22 GB/s | ~3 GB/s | **7×** |
| Mac i7 | ~33 GB/s | ~4 GB/s | **8×** |
| Asus Pentium | ~9 GB/s | ~0.9 GB/s | **10×** |

Auto-vectorization is not an optimization — it is the baseline. A single unpredictable branch in the inner loop negates tile size, memory layout, thread count, and ISA differences simultaneously.

### 12.3 Write/Read Asymmetry — Every Machine

Write-allocate RFO policy: before writing to a cold cache line, the CPU must read the full 64-byte line first (to avoid overwriting neighboring bytes it doesn't own). Every write miss generates a read. Store buffer (~50–80 entries) drains this traffic; when full, the pipeline stalls.

| Machine | Read ceiling | Write ceiling | Write/read ratio |
|---|---|---|---|
| M4 Max | 259 GB/s | 104 GB/s | 0.40 |
| Win Nitro | 43 GB/s | 6.8 GB/s | 0.16 |
| Mac i7 | 68 GB/s | 18 GB/s | 0.26 |
| Asus Pentium | 18 GB/s | 6.0 GB/s | 0.33 |

Write ceiling saturates at fewer threads than read ceiling on every machine. M4 write saturates at 8 threads (104 GB/s), read at 10 threads (259 GB/s) — 3× asymmetry.

### 12.4 TLB Working Set Sweep — Cross-System

M4's 16 KB pages give 4× TLB reach vs x86's 4 KB pages. L1 DTLB reach: M4 = 4 MB, x86 = 256 KB.

| Working set | M4 BW | Asus BW | Mac i7 BW | Win BW |
|---|---|---|---|---|
| 16 KB | 8.13 GB/s | 1.41 GB/s | 3.53 GB/s | 1.88 GB/s |
| 256 KB | 8.31 GB/s | 0.93 GB/s | 3.22 GB/s | 2.29 GB/s |
| 1 MB | 6.55 GB/s | **0.19 GB/s** | 2.63 GB/s | 2.03 GB/s | ← x86 TLB exhausted |
| 4 MB | 6.87 GB/s | 0.12 GB/s | 2.05 GB/s | 1.37 GB/s | ← M4 L1 DTLB full |
| 16 MB | 3.30 GB/s | 0.11 GB/s | 0.79 GB/s | 0.52 GB/s |
| 256 MB | 1.31 GB/s | 0.03 GB/s | 0.31 GB/s | 0.33 GB/s |

The 26 MB heightmap sits in full TLB-miss territory on x86 but is partially covered by M4's L2 TLB (~48 MB reach).

### 12.5 SoA Advantage Scales With Bandwidth Pressure

| Machine | Single-thread | Parallel |
|---|---|---|
| M4 Max | 1.00× | 1.13× |
| Win Nitro | 1.14× | **2.3×** |
| Asus Pentium | 1.0× | **2.7×** |

SoA advantage is invisible when compute-bound (OOO hides load latency). Under parallel bandwidth pressure, SoA loads 1 float per pixel instead of 3 → 3× less data.

### 12.6 PCIe Readback — The GPU fps Bottleneck on Discrete Hardware

| System | GPU compute | PCIe readback (3.4 MB) | Total | fps |
|---|---|---|---|---|
| M4 Max (unified) | ~22 ms | 0 ms | ~22 ms | **46.4 fps** |
| Win GTX 1650 (PCIe ×4) | ~20 ms | ~47 ms | ~67 ms | **15.2 fps** |
| Mac Intel i7 (iGPU) | — | 0 ms | ~85 ms | **11.8 fps** |
| Asus Pentium (iGPU) | — | 0 ms | ~222 ms | **4.5 fps** |

GTX 1650 computes frames faster than M4 (~20 ms vs ~22 ms) yet achieves 3× lower fps because PCIe readback costs 47 ms — more than the render itself. The fps number measures PCIe bandwidth, not shader throughput. Fix: wgpu Surface (swap chain) eliminates readback entirely; expected GTX 1650 fps ≈ 50.

### 12.7 GPU vs CPU Win Conditions

| Workload | Winner | Reason |
|---|---|---|
| Raymarching (17.7× GPU) | GPU | Embarrassingly parallel; massive thread-level parallelism |
| Shadow sweep (17× CPU) | CPU | Serial running-max dependency; GPU clock lower, no ILP benefit |

Multi-frame per-frame: CPU parallel 1730 ms → GPU scene 98 ms (17.7× speedup).
