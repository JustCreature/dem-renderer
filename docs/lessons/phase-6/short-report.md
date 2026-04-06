# Phase 6 — Experiment Matrix: Reference Card

**Hardware**: Apple M4 Max · 10 perf cores · 400 GB/s DRAM · 128 KB L1D · 16 MB L2 · 48+ MB SLC
**Dataset**: USGS SRTM N47E011, 3601×3601, ~26 MB i16 / ~52 MB f32
**All results cold cache (100 MB evict) unless noted. Date: 2026-04-06**

---

## Experiment Quick Reference

| # | Variable | Winner | Ratio | Root cause |
|---|---|---|---|---|
| 1 | Tile size (T=32…256) | row-major baseline | baseline 6× faster than all tiled | `continue` blocks auto-vec |
| 2 | Thread count with writes | 12 threads | 9× vs 1T | write path saturates at ~8 cores |
| 3 | Thread count read-only | 10 threads | 23× vs 1T | DRAM read ceiling at ~259 GB/s |
| 4 | AoS vs SoA | ~tie / SoA 1.1× (parallel) | 1.00× / 1.13× | single-thread is compute-bound; parallel barely bandwidth-limited |
| 5 | Morton vs row-major tiled | tie | 1.00× | scalar loop bottleneck; OOO hides L2 latency |
| 6 | Software prefetch (D=4…64) | D=64 | +14% only | M4 Max ROB (~600 instr) already issues speculative loads |
| 7 | NEON accumulator count | 8 acc | 5.1× over 1-acc, 3.0× over auto-vec | serial dep chain; 4 acc crosses to memory-bound |
| 8 | Ray packet gather width | 4-wide adjacent cols | 3× pixels/ns | 4 values per cache line; OOO absorbs strided cost |
| 9 | TLB working set | ≤4 MB | knees at 4 MB and 16 MB | 16KB pages → L1 DTLB=4 MB, L2 DTLB≈48 MB |

---

## Experiment Tables

### Exp 1 — Tile Size Sweep
5-point stencil (i16→f32), 3601×3601, 181 MB/run

| Variant | GB/s |
|---|---|
| row-major (auto-vec 8-wide NEON) | 60–72 |
| tiled T=32 | 10.13 |
| tiled T=64 | 10.25–10.76 |
| tiled T=128 | 10.67–11.06 |
| tiled T=256 | 10.84–11.01 |

**Key**: tile size is irrelevant when the inner loop is scalar. The `continue` border-skip branch blocks LLVM vectorisation regardless of tile size or access frequency.

### Exp 2 — Thread Scaling (with writes)
Rayon `par_chunks_mut`, cold cache

| Threads | GB/s |
|---|---|
| 1 | 11.2–11.5 |
| 2 | 22.7–22.9 |
| 4 | 37.9–43.3 |
| 6 | 42.9–66.9 |
| 8 | 83.9–85.2 |
| 10 | 85.8–96.6 |
| 12 | 100.9–103.4 |

Note: 1T rayon = 11 GB/s (not 60 GB/s) because the `FnMut` closure boundary also blocks auto-vectorisation.

### Exp 3 — Thread Scaling (read-only)
Per-thread i64 accumulator, no output buffer

| Threads | GB/s (read-only) | GB/s (with writes) | Ratio |
|---|---|---|---|
| 1 | 31.9–34.8 | 11.2 | ~3× |
| 2 | 64.3 | 22.7 | 2.8× |
| 4 | 120.0 | 37.9 | 3.2× |
| 8 | 226.3 | 85.2 | 2.7× |
| 10 | **259.0** | 96.6 | 2.7× |
| 12 | 246.9 | 100.9 | 2.4× |

**DRAM read ceiling**: ~259 GB/s at 10 threads (~65% of 400 GB/s peak).
**Write path is ~3× narrower**: store buffer → L1D writeback → L3 → DRAM is a narrower physical path than the read fill-buffer pipeline.

### Exp 4 — AoS vs SoA
Normal map, 12 Mpix; AoS = 155 MB total; SoA = 3×51 MB

| Kernel | SoA GB/s | AoS GB/s | Ratio |
|---|---|---|---|
| dot(normal, sun), 12 B/pix | 22.4–24.2 | 22.4–24.1 | 1.00× |
| nz-only, 1 thread | 8.0–8.3 | 8.1–8.4 | 1.00× |
| nz-only, 10 threads | 66.0 (51 MB) | 58.3 GB/s logical (175 MB actual) | 1.13× |

**Single-thread tie**: serial `fadd` dep chain (3-cycle latency) is the bottleneck regardless of bytes loaded.
**Parallel 1.13×**: memory-bound at 10T, but M4 Max's 400 GB/s bus serves both without saturation. On a bandwidth-saturated CPU the ratio would approach 3×.

### Exp 5 — Morton vs Row-major Tiling
5-point stencil, T=64, cold cache

| Variant | GB/s |
|---|---|
| row-major (vectorised) | 64–68 |
| tiled row-major T=64 | 10.25–10.40 |
| tiled Morton T=64 | **10.43** (1.00×) |

Morton reduces N/S tile distance: 456 KB → 8–32 KB. Effect = zero. The OOO ROB (~600 entries, ~60 loop iterations look-ahead) absorbs the ~12-cycle L2 latency inside compute time. Scalar loop is the floor.

### Exp 6 — Software Prefetch
256 MB array, 64M random reads (pre-shuffled indices)

| Variant | GB/s | vs baseline |
|---|---|---|
| no prefetch | 1.20 | — |
| prfm D=4 | 1.28 | +7% |
| prfm D=16 | 1.31 | +9% |
| prfm D=64 | **1.37** | **+14%** |

**Why so little**: M4 Max ROB (≈600 instructions) already speculatively issues loads 60+ iterations ahead for simple loops. Explicit `prfm` is redundant on aggressive OOO machines. It helps on in-order or small-ROB cores.

### Exp 7 — NEON Multiple Accumulators
SoA dot product, 12 Mpix, 155 MB total

| Variant | GB/s | Bottleneck |
|---|---|---|
| auto-vec scalar (implicit 1 acc) | 23.7 | compute — fadd dep chain, 3-cycle lat |
| NEON explicit 1 acc | **18.0** (slower) | compute — vfmaq dep chain, 4–5 cycle lat |
| NEON explicit 4 acc | **71.0** | DRAM — both FP ports busy every cycle |
| NEON explicit 8 acc | **120.0** | SLC — data partially in 48+ MB SLC |

**Transition model**:
```
1 acc  → compute-bound (fadd latency)
4 acc  → DRAM-bound   (~65 GB/s, both FP ports saturated)
8 acc  → SLC-bound    (~120 GB/s, eviction incomplete, data hits SLC)
```

**M4 Max has 2 FP/SIMD ports per core.** With K ≥ 4 independent accumulators, both ports stay busy every cycle.

### Exp 8 — Ray Packet Gather Cost
2M random positions, 3601×3601 heightmap (25 MB)

| Variant | GB/s | ns/step | Pixels/step | ns/pixel |
|---|---|---|---|---|
| 1-wide scalar | 1.31 | 1.70 ns | 1 | 1.70 ns |
| 4-wide adjacent cols | 3.84 | 2.30 ns | 4 | **0.57 ns** |
| 4-wide strided rows | 2.90 | 3.06 ns | 4 | **0.76 ns** |
| 4-wide fully random | ~2.1 (reported 8.35 at 4× bytes) | 4.1 ns | 4 | **1.03 ns** |

Cache line geometry (64 bytes = 32 × i16, row = 7202 bytes):
- Adjacent cols: 4 values in 1 cache line → **1 miss for 4 pixels**
- Strided rows: 4 separate rows → **4 misses**, but OOO issues them in parallel
- Random: 4 independent positions → MLP > 1-wide serial chain

**Why Phase 4 NEON = scalar 1.00×**: gather alone is 3× faster, but ray divergence after ~50 steps converts adjacent-column access to random. Compute per step (height compare, ray advance) also doesn't cleanly vectorise across diverged rays.

### Exp 9 — TLB Working Set Sweep
1M random reads (4 bytes each), macOS/M4 Max (16KB base pages)

| Working set | 16KB pages | GB/s | Layer |
|---|---|---|---|
| 16 KB | 1 | 8.20 | L1 cache + L1 DTLB |
| 64 KB | 4 | 7.77 | L2 cache + L1 DTLB |
| 256 KB | 16 | 8.20 | L3/SLC + L1 DTLB |
| 1 MB | 64 | 7.51 | L3/SLC + L1 DTLB |
| **4 MB** | **256** | **6.40** | ← L1 DTLB full (~256 entries × 16KB) |
| **16 MB** | **1024** | **3.77** | ← L2 TLB pressure |
| 64 MB | 4096 | 1.91 | L2 TLB miss + page walks |
| 256 MB | 16384 | 1.38 | page walks + DRAM latency |

**macOS vs x86 page sizes**:

| Metric | x86 Linux (4KB) | M4 Max (16KB) |
|---|---|---|
| L1 DTLB entries | ~64 | ~256 |
| L1 DTLB coverage | ~256 KB | **~4 MB** |
| L2 TLB entries (est.) | ~1500 | ~3000 |
| L2 TLB coverage | ~6 MB | **~48 MB** |

**Renderer implication**: 26 MB heightmap = 1625 pages → fits in L2 TLB (≈48 MB) → no page walks during random DEM access. 256 MB benchmark array (Exp 6) = 16384 pages → spills to page walks → ~1.2 GB/s baseline includes page-walk overhead.

---

## Lessons

1. **Vectorisation gates everything.** A single `continue` in the inner loop cut throughput 6× and made tile size, Morton ordering — everything else — irrelevant. Control-flow visibility to the compiler is the most important property of a hot loop.

2. **The write path is ~3× narrower than the read path.** Write-heavy parallel kernels saturate at ~8 cores (100 GB/s); read-only kernels saturate at ~10 cores (259 GB/s). This is a real M4 Max hardware asymmetry: store-buffer → writeback → DRAM vs fill-buffer → DRAM prefetch.

3. **Serial reduction chains are compute-bound, not memory-bound.** `sum += data[i]` is limited by fadd dep-chain latency (3–5 cycles/result), not DRAM speed. The fix is multiple independent accumulators. NEON with 1 acc is slower than scalar, not faster.

4. **OOO execution makes software prefetch largely redundant on M4 Max.** The ~600-instruction ROB issues speculative loads 60+ iterations ahead for simple loops. `prfm` adds only 14% on top. Explicit prefetch only pays off on in-order or small-ROB processors, or for pointer-chasing patterns where the next address isn't visible until the current load completes.

5. **macOS 16KB pages give 4× TLB coverage vs x86.** L1 DTLB ≈ 4 MB, L2 TLB ≈ 48 MB. The 26 MB heightmap fits in L2 TLB. The renderer never pays page-walk latency for DEM access.

6. **Ray packet memory gain (3×) is absorbed by compute divergence.** Adjacent-column gathers share cache lines and deliver 3× pixels/ns in isolation. But rays diverge within ~50 steps, converting access to strided/random. The compute per step (height compare, ray advance) doesn't vectorise cleanly across diverged rays. Net gain for full render: 1.00×.

---

## Wrong Hypotheses Corrected

| Hypothesis | Measured | Cause of mismatch |
|---|---|---|
| Tile size should affect tiled stencil | all T=32–256 within noise | scalar loop (from `continue`) is the floor |
| Morton reduces N/S distance → speedup | Morton = row-major, 1.00× | OOO ROB absorbs L2 latency inside compute |
| Removing writes → small gain (~20%) | ~3× faster | write path is structurally narrower than read path |
| prfm D=64 → 8–16× random read speedup | +14% only | ROB already provides hardware MLP for simple loops |
| NEON 1-acc ≥ auto-vec scalar | NEON 1-acc is 24% slower | vfmaq latency 4–5 cycles vs fadd 3 cycles |
| AoS nz-only → 3× slower than SoA | single-thread 1.00×; 10T 1.13× | compute-bound (dep chain); bus not saturated |
