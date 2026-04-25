# Cross-Platform Benchmark Analysis Report
**dem_renderer — Phases 0–6 Full Results**
*Generated: 2026-04-09*

---

## Systems Under Test

| ID | Machine | CPU | ISA | Cores | RAM | GPU |
|---|---|---|---|---|---|---|
| **m4_mac** | MacBook / Mac Studio | Apple M4 Max | AArch64 | 14 (14 logical) | 36 GB | Apple M4 Max (integrated, unified) |
| **mac_intel_i7** | MacBook Pro | Intel Core i7-1068NG7 @ 2.30 GHz | x86_64 | 4P / 8HT | 32 GB | Intel Iris Plus (integrated) |
| **win_nitro** | Acer Nitro laptop | Intel Core i5-9300H @ 2.40 GHz | x86_64 (v3) | 4P / 8HT | 31 GB | **NVIDIA GTX 1650** (discrete) + Intel UHD 630 |
| **asus_pentium** | ASUS X540SA laptop | Intel Pentium N3700 @ 1.60 GHz | x86_64 | 4 / 4 | 3 GB | Intel HD 405 (integrated, weak) |

**Key architectural differences:**
- M4 Max: 16 KB OS pages, ~400 GB/s unified memory bandwidth, 128 KB L1D per perf-core, no NUMA
- Intel x86 systems: 4 KB OS pages, DDR4 DRAM (much narrower bandwidth), separate L1/L2/L3 hierarchy
- GTX 1650: dedicated GDDR5, PCIe connection — compute is fast but readback is the bottleneck
- Pentium N3700: Silvermont microarch (2014), 3 GB RAM saturates easily, no AVX2

---

## 1. Memory Bandwidth

### Sequential Bandwidth

| Benchmark | M4 Mac | Mac i7 | Win GTX 1650 | Asus Pentium |
|---|---|---|---|---|
| SIMD sequential read | **27.3 GB/s** | 13.3 GB/s | 21.0 GB/s | N/A |
| Scalar sequential read | 6.7 GB/s | 3.2 GB/s | 2.9 GB/s | 2.5 GB/s |
| Sequential write | 10.2 GB/s | 1.8 GB/s | 2.7 GB/s | 0.5 GB/s |
| Random read | 0.6 GB/s | 0.1 GB/s | 0.1 GB/s | ~0 GB/s |
| Random write | 0.5 GB/s | 0.2 GB/s | 0.1 GB/s | ~0 GB/s |

**Key observations:**

1. **M4 Max SIMD hits 27 GB/s sequential** — only 7% of the 400 GB/s theoretical peak from a single thread. The single-thread ceiling for SIMD reads is ~60–70 GB/s (confirmed in Phase 6 stencil). Full bandwidth requires 10–12 threads.

2. **Win GTX 1650 machine matches M4 at SIMD sequential** (21 GB/s vs 27 GB/s). This is counter-intuitive given the age gap — the i5-9300H has AVX2 and a fast L3, so single-thread SIMD bandwidth is competitive with M4 single-thread.

3. **Asus Pentium random read effectively 0** — wall clock was ~20 seconds for the benchmark. With only 3 GB RAM and a 256 MB test array, the system was thrashing virtual memory, reading from swap. This is not a DRAM bandwidth number — it's an SSD/eMMC read rate.

4. **Random-to-sequential ratio**: M4 0.022×, Mac i7 0.031×, Win 0.034×. All are in the same 20–50× penalty range for random vs sequential access — this is the universal DRAM/TLB/cache miss tax.

### Neighbour Sum (Phase 1)

| Benchmark | M4 Mac | Mac i7 | Win GTX | Asus |
|---|---|---|---|---|
| Row-major (auto-vec) | 24.0 GB/s | 18.4 GB/s | 24.8 GB/s | 1.3 GB/s |
| Tiled (get() abstraction) | 4.2 GB/s | 0.5 GB/s | 0.6 GB/s | 0.1 GB/s |

**The `get()` abstraction killed performance** on every system — not just M4. The Phase 1 lesson is universal: abstraction cost (decomposition of indices, indirect indexing) blocks auto-vectorisation and penalises every microarchitecture equally.

---

## 2. Normal Map Computation

| Benchmark | M4 Mac | Mac i7 | Win GTX | Asus |
|---|---|---|---|---|
| Scalar | 19.1 GB/s | 4.3 GB/s | 8.1 GB/s | 0.4 GB/s |
| Auto-vectorised | 20.6 GB/s | 4.3 GB/s | 9.5 GB/s | 0.4 GB/s |
| Parallel (rayon) | 54.7 GB/s | 14.0 GB/s | 20.1 GB/s | 0.4 GB/s |
| Tiled vectorised | 31.4 GB/s | 10.7 GB/s | 3.7 GB/s | 0.2 GB/s |
| Tiled parallel | **110.9 GB/s** | 11.2 GB/s | 12.3 GB/s | 0.2 GB/s |
| GPU compute | 0.069 s | 0.247 s | 0.116 s | 2.132 s |

**Key observations:**

1. **M4 tiled parallel is 110.9 GB/s — 2× over M4 plain parallel (54.7 GB/s).** The tiling benefit is only visible when all threads have good spatial locality (tiled input + tiled traversal). On M4 with its 400 GB/s memory and large L1D (128 KB), tiling pays off handsomely.

2. **Win GTX i5 tiled parallel is 12.3 GB/s — WORSE than plain parallel (20.1 GB/s).** The i5-9300H has only 4 physical cores with 32 KB L1D. The tiled parallel implementation likely serialises some access patterns through the small L1, and the write output path (row-major NormalMap) conflicts with tiled traversal.

3. **Asus Pentium: all normal variants ≈ 0.2–0.4 GB/s** — this CPU is memory-bandwidth limited at the hardware level. Silvermont has very limited SIMD execution width and extremely narrow DRAM interface.

4. **GPU normals on Asus takes 2.13 s** — Intel HD 405 (Braswell integrated, 2015) is slower than 4-core Pentium for compute. This is a low-power SoC GPU with far fewer ALUs than even the cheapest discrete card.

5. **Parallelism speedup**: M4 gets 2.9× (plain parallel) — bandwidth-limited above ~10 cores. i7 Mac gets 3.3×, Win gets 2.5×. Asus gets 1.0× (bandwidth already saturated at 1 thread because 3 GB RAM pressure dominates).

---

## 3. Shadow Sweep

| Benchmark | M4 Mac | Mac i7 | Win GTX | Asus |
|---|---|---|---|---|
| Scalar cardinal | 7.6 GB/s | 2.3 GB/s | 1.2 GB/s | 0.3 GB/s |
| SIMD cardinal | 8.5 GB/s | 2.6 GB/s | 2.7 GB/s | 0.3 GB/s |
| SIMD parallel cardinal | 7.0 GB/s | 7.5 GB/s | 5.5 GB/s | 0.3 GB/s |
| SIMD parallel azimuth (sunset) | **27.2 GB/s** | 2.0 GB/s | 1.5 GB/s | 0.1 GB/s |
| GPU shadow | 0.035 s | 0.205 s | 0.155 s | 0.996 s |

**Key observations:**

1. **M4 sunset azimuth SIMD parallel: 27.2 GB/s — best result among all shadow variants.** At sunset the sun direction is nearly east–west, which is the optimal DDA direction (short diagonal steps with good cache behaviour). The M4's large L1D and high bandwidth utilise this well.

2. **Win GTX i5 SIMD parallel cardinal (5.5 GB/s) vs azimuth (1.5 GB/s) — 3.7× degradation.** The diagonal strided access pattern on a smaller L1D/L2 causes heavy cache pressure. This 2–4× azimuth penalty is consistent with Phase 3 findings.

3. **GPU shadow is slow everywhere relative to CPU NEON parallel.** Shadow sweep has a serial running-maximum dependency per row — GPU can't parallelise this. All GPU cores stall waiting for predecessor values. CPU SIMD (vmaxq_f32/vmax) in a serial loop beats it because CPUs are optimised for exactly this kind of serial dependency chain.

4. **Mac i7 SIMD parallel (7.5 GB/s) > M4 parallel cardinal (7.0 GB/s)**. Surprising — the i7 has 8 logical cores vs 14 on M4, but the shadow kernel saturates memory bandwidth at 7–8 GB/s, and the i7's DDR4 bandwidth at full parallel utilisation is enough for this serial-dependency-limited kernel.

---

## 4. CPU Rendering

### 2000×900 Image

| Variant | M4 Mac | Mac i7 | Win GTX | Asus |
|---|---|---|---|---|
| Parallel scalar | **0.084 s** | 0.817 s | 1.262 s | 7.02 s |
| SIMD single-thread | 0.834 s | 3.088 s | 3.543 s | 27.22 s |
| SIMD parallel | 0.089 s | 0.840 s | 1.092 s | 7.06 s |
| SIMD ≈ scalar parallel | ≈1.0× | ≈1.0× | ≈1.2× | ≈1.0× |

**The SIMD gain is zero in every case.** Phase 4 analysis confirmed this: the 4-wide ray packet starts with adjacent columns (same cache line), but diverges after 10–50 steps. For the remaining ~460 of 506 average steps, each ray is on a different cache line. Gather cost (4× cache misses) exactly cancels the 4× SIMD arithmetic gain.

### 8000×2667 Image (21.3 Mpix)

| Variant | M4 Mac | Mac i7 | Win GTX | Asus |
|---|---|---|---|---|
| CPU parallel | 1.265 s | 12.93 s | 21.55 s | 124.53 s |
| GPU buffer | 0.136 s | 1.268 s | 0.352 s | 15.15 s |
| GPU texture | 0.138 s | 1.570 s | 0.375 s | 15.15 s |
| GPU combined | 0.108 s | 1.465 s | 0.343 s | 15.16 s |
| **GPU speedup** | **9.3×** | **10.2×** | **62.9×** | **8.2×** |

**The Win GTX 1650 achieves 62.9× GPU speedup** — by far the largest of any system. This is because:
- The i5-9300H is a weak CPU for this workload (4 cores, single-threaded raymarching is serial-heavy)
- The GTX 1650 is a proper discrete GPU with GDDR5 and many more parallel shader units
- Raymarching is embarrassingly parallel — no inter-pixel dependencies, perfect for GPU

**Mac i7 and M4 get only 9–10× speedup** because the Mac M4's CPU is already very fast (unified memory helps enormously), and the Mac i7's integrated Iris Plus is relatively strong for an iGPU (Gen 11 graphics).

**GPU texture is slower than buffer on all systems** — the stripe-like ray access pattern in raymarching doesn't benefit from the 2D texture cache. The sampler unit adds latency without providing cache hits.

---

## 5. Multi-frame Benchmark (GpuScene vs separate)

| System | GPU separate (ms/f) | GPU combined (ms/f) | GPU scene (ms/f) | Scene speedup |
|---|---|---|---|---|
| M4 Mac | 133.3 | 118.9 | **97.6** | 1.37× |
| Mac i7 | 1972.4 | 1984.6 | 1802.1 | 1.09× |
| Win GTX | 495.6 | 453.5 | 409.3 | 1.21× |
| Asus | 20984.6 | 20767.1 | 19904.0 | 1.05× |

**GpuScene speedup is largest on M4 (1.37×)** because M4 has the lowest base render time, so the savings from eliminating per-frame uploads (writing only 128 bytes vs full resource re-upload) matter most proportionally. On Asus, the GPU is so slow that the render time dominates everything — no speedup possible.

**The multi-frame bottleneck on all systems is the CPU readback** (~3.4 MB pixel buffer transferred PCIe per frame). This is why the FPS numbers are so much worse than a viewer would be:
- M4: 97.6 ms/frame = 10.2 fps → actual GPU compute is only ~10 ms (10× faster if readback eliminated)
- Win GTX: 409.3 ms/frame — includes both GPU compute (~65 ms) and PCIe readback (~47 ms) plus overhead

---

## 6. FPS Benchmark (1600×533, 30-frame pan)

| System | CPU fps | GPU fps | GPU speedup | Bottleneck |
|---|---|---|---|---|
| M4 Mac | 15.07 fps | **46.42 fps** | 3.1× | GPU compute (~21 ms) |
| Mac i7 | 1.43 fps | 4.64 fps | 3.2× | GPU compute + readback |
| Win GTX 1650 | 0.88 fps | **15.19 fps** | 17.2× | PCIe readback (~47 ms) |
| Asus Pentium | 0.15 fps | 0.68 fps | 4.6× | Everything (ancient GPU) |

**Win GTX 1650 achieves 15.19 fps GPU — limited by PCIe readback, not GPU compute.** From nvidia-smi analysis: GPU SM utilisation is ~30% during the benchmark. The GPU computes in ~20 ms, then waits 47 ms for CPU readback. If readback were eliminated (Surface/swapchain viewer), theoretical fps = 1000/20 = 50 fps.

**M4 Mac achieves 46.42 fps GPU** — the best absolute fps, benefiting from unified memory (no PCIe, readback is ~8 ms instead of 47 ms) and fast GPU compute.

**GPU speedup correlates with CPU weakness, not GPU strength:**
- Asus (4.6×): both CPU and GPU are slow, but GPU is proportionally less bad
- Win GTX (17.2×): weak CPU × strong discrete GPU = large ratio

---

## 7. Phase 6 Experiments — Cross-System Analysis

### Exp 1: Tile Size Sweep

| Layout | M4 | i7 Mac | Win GTX | Asus | Ratio (row_major / best_tiled) |
|---|---|---|---|---|---|
| Row-major | 71.25 | 26.38 | 17.84 | 5.80 | — |
| Tiled T=32 | 10.59 | 3.67 | 1.81 | 0.61 | — |
| Tiled T=64 | 10.59 | 2.33 | 1.92 | 0.62 | — |
| Tiled T=128 | 11.10 | 3.61 | 1.67 | 0.63 | — |
| Tiled T=256 | 11.06 | 3.60 | 2.24 | 0.60 | — |
| **Tiling penalty** | **6.7×** | **7.2×** | **7.9×** | **9.4×** | consistent across all |

**The ~7–10× tiling penalty is universal.** The root cause is a `continue` branch inside the tiled loop that prevents auto-vectorisation — not a cache hierarchy issue. This is the single most important lesson of Phase 6: **vectorisation gates everything**. No amount of cache-conscious layout helps if the vectoriser is blocked.

Tile size has almost no effect on any system — T=32 to T=256 are all within ±10% because once the loop structure blocks vectorisation, the L1/L2/L3 hierarchy is no longer the bottleneck.

### Exp 2 vs 3: Thread Scaling — Write Path vs Read Path

| System | Peak (writes) | Peak (reads) | Write/Read ratio |
|---|---|---|---|
| M4 Mac | 104.6 GB/s | 264.4 GB/s | **0.40** |
| Win GTX i5 | 6.8 GB/s | 73.5 GB/s | **0.09** |
| Mac i7 | 12.1 GB/s | 51.5 GB/s | **0.23** |
| Asus Pentium | 2.4 GB/s | 4.6 GB/s | **0.52** |

The write path is dramatically narrower than the read path on all systems. This is caused by:
- **Store buffer**: only ~20 stores can be in-flight simultaneously (ROB constraint)
- **Write-allocate**: every write to a cache-missing line causes a read-for-ownership (RFO) before the write — effectively doubling DRAM traffic
- **M4 write path saturates at ~8 threads (87.8 GB/s)** while reads continue scaling to 10+ threads (264.4 GB/s) — the asymmetry is 2.5× at saturation

On Win GTX i5 the write/read ratio is only 0.09 — 4 cores with hyperthreading can't drive DRAM writes efficiently; the store buffer fills faster than the DRAM write path can drain it.

### Exp 4: AoS vs SoA

| System | AoS dot result | SoA advantage (parallel nz-only) | Verdict |
|---|---|---|---|
| M4 Mac | AoS ≈ SoA (1.02×) | 1.0× | compute-bound; layout irrelevant |
| Mac i7 | AoS 2.47× FASTER | 1.8× SoA advantage at parallel | anomalous (cache effects, compiler) |
| Win GTX i5 | SoA 1.83× faster | 2.3× SoA advantage | SoA wins under bandwidth pressure |
| Asus | SoA 1.18× faster | 2.7× SoA advantage | biggest gap — bandwidth-starved |

**The Mac i7 AoS-is-faster result for the full dot product is anomalous.** This is likely a compiler artefact — when the compiler sees interleaved [nx, ny, nz] access for a dot product, it can reorder/fuse loads more aggressively than for SoA with separate arrays. The hardware prefetcher on the i7 may also do better with a single stream than three independent streams.

**SoA advantage is largest on bandwidth-starved systems (Asus 2.7×)**. When memory bandwidth is the limiting resource, wasting 2/3 of every cache line (AoS nz-only) doubles effective memory pressure. SoA reads only the needed component.

### Exp 5: Morton vs Row-major Tiling

Morton Z-order vs tiled row-major = **1.00–1.04× everywhere**. Morton ordering should help N/S cache locality, but the bottleneck is the scalar `continue` branch — not tile-distance. When the vectoriser is blocked, the CPU is compute-bound on branch prediction overhead, and memory layout is irrelevant.

### Exp 6: Software Prefetch (Random Access)

| System | No-prefetch | Best prefetch | Gain |
|---|---|---|---|
| M4 Mac | 1.32 GB/s | 1.48 GB/s (D=64) | +12% |
| Win GTX i5 | 0.34 GB/s | — | — |
| Mac i7 | 0.32 GB/s | — | — |
| Asus | 0.03 GB/s | — | — |

**Software prefetch gains are marginal on M4 (+12% max).** The Out-of-Order engine (ROB ~600 instructions on M4) can track enough outstanding cache misses to keep DRAM busy without explicit hints. Only at very long distances (D=64, 64 cachelines ahead) does explicit prefetch help by pushing misses far enough ahead to overlap.

The sequential/random gap (68 GB/s vs 1.32 GB/s = 51×) is the real lesson — random access to a 268 MB array is purely latency-bound (one DRAM RTT per access, ~80 ns on M4 = ~1.3 GB/s matches measured).

### Exp 7: Multiple Accumulators (NEON)

M4 Mac only:

| Implementation | GB/s | Speedup vs auto-vec |
|---|---|---|
| Auto-vec scalar | 23.4 | 1.0× |
| NEON 1 accumulator | 18.1 | 0.77× |
| NEON 4 accumulators | 73.9 | 3.2× |
| NEON 8 accumulators | 122.5 | 5.2× |

**Single-accumulator NEON is slower than auto-vec** — the vfmaq (fused multiply-add) has 4–5 cycle latency. A single accumulator creates a 4-cycle serial dependency chain. Auto-vec uses multiple implicit accumulators via loop unrolling.

**4 accumulators breaks the dependency chain: 73.9 GB/s** — now memory-bound (exceeds 68 GB/s single-thread ceiling because the 155 MB working set partially fits in L2 cache during the warm pass).

**8 accumulators: 122.5 GB/s** — significantly above the cold-cache bandwidth ceiling, confirming L2 cache hit contribution.

### Exp 8: Ray Packet Gather

| System | 1-wide scalar | 4-wide adjacent | 4-wide strided | 4-wide random |
|---|---|---|---|---|
| M4 Mac | 1.42 | 3.49 (2.5×) | 2.54 (1.8×) | 8.10 (5.7×) |
| Win GTX | 0.24 | 0.77 (3.2×) | 0.32 (1.3×) | 1.00 (4.2×) |
| Mac i7 | 0.31 | 0.76 (2.5×) | 0.34 (1.1×) | 1.27 (4.1×) |
| Asus | 0.05 | 0.08 (1.6×) | 0.05 (1.0×) | 0.21 (4.2×) |

Adjacent columns (same cache line) give 2.5–3.2× over scalar. But in real raymarching, rays diverge after ~10–50 steps and most of the 506 average steps are in the strided/random regime, where the gain disappears. Phase 4 confirmed: SIMD gather gain ≈ 1.0× net in actual rendering.

### Exp 9: TLB Working Set Sweep

| System | L1 DTLB capacity | L2 TLB knee | Page size | L2 TLB capacity |
|---|---|---|---|---|
| M4 Mac | ~4 MB | ~16 MB | 16 KB | ~48 MB |
| x86 systems | ~1 MB | ~4–8 MB | 4 KB | ~4–16 MB |

The M4's 16 KB OS pages give **4× larger TLB coverage** than x86 4 KB pages. The heightmap (26 MB) fits comfortably inside the M4's L2 TLB (48 MB capacity), meaning raymarching never triggers full page-table walks. On x86 systems with 4 KB pages, the 26 MB heightmap requires 6500 TLB entries — exceeding typical 2048-entry L2 TLBs.

The Asus Pentium TLB is exhausted at 256 KB — extremely small TLB capacity on Silvermont cores.

---

## 8. Absolute Performance Summary

### Normalised Throughput (higher = better, M4 = 1.0×)

| Benchmark | M4 Mac | Win GTX | Mac i7 | Asus |
|---|---|---|---|---|
| Sequential bandwidth | 1.0× | 0.43× | 0.48× | 0.04× |
| Normal computation (parallel) | 1.0× | 0.18× | 0.13× | 0.004× |
| Shadow sweep (best variant) | 1.0× | 0.28× | 0.07× | 0.004× |
| CPU rendering 8000×2667 | 1.0× | 0.059× | 0.098× | 0.010× |
| GPU rendering 8000×2667 | 1.0× | 0.315× | 0.074× | 0.007× |
| GPU combined rendering | 1.0× | 0.315× | 0.074× | 0.007× |
| FPS GPU (1600×533) | 1.0× | 0.33× | 0.10× | 0.015× |

### Key Absolute Numbers

| Metric | Best system | Value |
|---|---|---|
| Peak sequential bandwidth | M4 Mac (SIMD, 12 threads) | ~264 GB/s |
| Peak SIMD single-thread bandwidth | M4 Mac | 27.3 GB/s |
| Fastest GPU FPS | M4 Mac | 46.42 fps |
| Fastest GPU FPS (discrete GPU) | Win GTX 1650 | 15.19 fps |
| Best GPU speedup vs CPU | Win GTX 1650 | 17.2× (FPS) / 62.9× (big render) |
| Fastest normal computation | M4 Mac tiled parallel | 110.9 GB/s |
| Fastest shadow sweep | M4 Mac SIMD parallel sunset | 27.2 GB/s |
| Slowest system overall | Asus Pentium N3700 | ~100× slower than M4 for rendering |

---

## 9. Cross-Cutting Conclusions

### 1. Vectorisation gates everything — the universal lesson
The Phase 6 tile size sweep showed the same ~7–10× penalty on every system (M4 to Asus). The cause is a `continue` branch in the tiled loop that blocks auto-vectorisation. Cache topology, page size, and SIMD width become irrelevant when the vectoriser can't fire. **Fix the vectorisation barrier first.**

### 2. PCIe readback is the GPU fps ceiling
The Win GTX 1650 benchmarks reveal a system with ~20 ms GPU compute but ~47 ms PCIe readback — making 67 ms/frame (15 fps) despite the GPU being 17× faster than the CPU. Eliminating readback via a wgpu Surface/swapchain viewer would push it to ~50 fps. This is the motivation for Phase 7 viewer.

### 3. Memory hierarchy determines architecture suitability
- **M4 unified memory** eliminates PCIe entirely for GPU, reduces TLB pressure (16 KB pages), and provides 400 GB/s to all agents. This is why M4 sees the best GPU fps despite not having a discrete GPU.
- **x86 discrete GPU** (Win GTX 1650) wins on raw GPU throughput but loses fps to PCIe latency.
- **x86 integrated GPU** (Mac i7, Asus) is consistently 3–20× slower than M4 GPU for the same workload.

### 4. SoA advantage scales with bandwidth pressure
SoA is 1.0× on M4 (compute-bound, memory has headroom), 1.8–2.3× on Intel systems (bandwidth-limited), and 2.7× on Asus (bandwidth-starved). Design data layouts for the most constrained target system.

### 5. Parallelism efficiency reveals the write bottleneck
Write-path DRAM bandwidth is consistently 40–90% narrower than read-path on all systems (0.40 on M4 to 0.09 on i5). This is store-buffer congestion + write-allocate RFO traffic. Shadow computation (read-dominated with one write per pixel per row) scales better than stencil (read+write per pixel).

### 6. Software prefetch is architecturally redundant on modern OOO CPUs
M4's ~600-instruction ROB provides enough in-flight miss coverage for predictable strides. Explicit `prfm` gains only 12% at best. On Asus (tiny ROB on Silvermont), the gain would theoretically be larger but the system is so bandwidth-limited that prefetch can't help either.

### 7. GPU beats CPU only for embarrassingly parallel work
- **Raymarching**: GPU wins 8–63× (no inter-pixel dependencies, 1000s of parallel shader threads)
- **Shadow sweep**: CPU wins (serial running-max dependency, GPU stalls on data hazards)
- **Normals**: mixed — GPU wins in absolute time but not per-core efficiency; CPU tiled parallel on M4 (110.9 GB/s) beats GPU normals (which takes 0.069 s ≈ 3.7 GB/s effective)

---

## 10. Recommendations for Phase 7

1. **Implement the `--view` wgpu Surface viewer** (planned) — this eliminates the PCIe readback bottleneck entirely on Win GTX 1650 (15 fps → ~50 fps) and improves M4 fps (46 → ~100+ fps ceiling).

2. **Fix the tiled loop vectorisation** — the 7–10× penalty is left on the table everywhere. Remove the `continue` guard or restructure as a masked SIMD operation.

3. **Use GPU timestamp queries** instead of wall-clock for bench_fps — this isolates pure shader time from readback noise. Expected: GTX 1650 shader = ~20 ms (50 fps shader ceiling), readback = ~47 ms (measured by difference).

4. **Do not prioritise software prefetch** — the ROB handles it on M4 and Intel Tiger Lake/Comet Lake. Focus on access-pattern restructuring instead.

5. **AoS-to-SoA conversion matters most on weak systems** — if targeting Asus-class hardware, SoA gives a 2.7× free speedup for memory-bound kernels.

---

*CSV data files in `docs/benchmark_results/report_1/`:*
- `systems.csv` — hardware profiles
- `memory_bandwidth.csv` — Phase 0–1 bandwidth results
- `normals.csv`, `shadows.csv` — Phase 2–3 results
- `rendering.csv`, `fps_benchmark.csv` — Phase 4–5 rendering results
- `phase6_exp1_tile_sweep.csv` through `phase6_exp9_tlb.csv` — Phase 6 experiment matrix

---

## Glossary

| Term | Full name | Short definition |
|---|---|---|
| <abbr title="Translation Lookaside Buffer">**TLB**</abbr> | Translation Lookaside Buffer | Hardware cache (~64–3000 entries) storing virtual→physical address translations. Hit = 1 cycle. Miss = page-table walk = 50–200 cycles. |
| **L1 DTLB** | Level-1 Data TLB | First-level TLB, smallest and fastest. M4: 256 entries × 16 KB = 4 MB coverage. x86: 64–256 entries × 4 KB = 256 KB–1 MB. |
| **L2 TLB** | Level-2 TLB | Larger, slower fallback TLB. M4: ~3000 entries × 16 KB = ~48 MB coverage. x86: ~1024–2048 entries × 4 KB = ~4–8 MB. |
| **page walk** | hardware page-table walk | CPU walks PGD→PUD→PMD→PTE in RAM when TLB misses. Each level is a potential DRAM read. Total cost ~50–200 cycles. |
| **PGD/PUD/PMD/PTE** | Page Global/Upper/Middle Dir, Page Table Entry | The 4 levels of the x86-64 page table tree. PGD address stored in CPU register CR3. PTE is the leaf containing the physical page address. |
| <abbr title="Read For Ownership">**RFO**</abbr> | Read For Ownership | Before writing to a cache-missing line, the CPU fetches the full 64-byte line first (to avoid overwriting neighbouring bytes). Every cold write causes a read — doubling DRAM traffic. |
| <abbr title="Last Level Cache">**LLC**</abbr> | Last Level Cache | Cache level closest to DRAM, shared by all cores. L3 on x86 (8–32 MB). SLC on M4 (32+ MB), shared with GPU and Neural Engine. |
| <abbr title="System Level Cache — Apple's name for the LLC on M4">**SLC**</abbr> | System Level Cache | Apple's LLC on M4 Max. 32+ MB, shared between all CPU cores, GPU, Neural Engine, and media engines via the SoC fabric. |
| <abbr title="Peripheral Component Interconnect Express">**PCIe**</abbr> | Peripheral Component Interconnect Express | Bus connecting CPU to discrete GPU. Gen3 ×16 ≈ 16 GB/s. Reading 3.4 MB pixels back to CPU costs ~47 ms in practice on GTX 1650. |
| <abbr title="Single Instruction Multiple Data">**SIMD**</abbr> | Single Instruction Multiple Data | One instruction operates on multiple elements simultaneously. ARM NEON = 4×f32. x86 AVX2 = 8×f32. |
| **NEON** | ARM Advanced SIMD | ARM's 128-bit SIMD: 4×f32, 4×i32, 8×i16, or 16×i8 per instruction. Used for vmaxq_f32 (shadow running-max) and vfmaq_f32 (dot products). |
| **AVX2** | Advanced Vector Extensions 2 | x86 256-bit SIMD: 8×f32 per instruction. Available on Win GTX (i5-9300H) and Mac i7; not on Asus Pentium N3700 (no AVX2 in Silvermont). |
| <abbr title="Struct of Arrays">**SoA**</abbr> | Struct of Arrays | Data layout: each field is a separate array [nx,nx,...][ny,ny,...]. Reading only nz loads 1/3 the cache lines vs AoS. |
| <abbr title="Array of Structs">**AoS**</abbr> | Array of Structs | Data layout: fields interleaved per element [{nx,ny,nz},...]. Reading only nz wastes 2/3 of every cache line loading nx and ny. |
| **stencil** | stencil computation | Kernel where output[i] depends on a fixed neighbourhood around i. The 5-point 2D stencil reads N/S/E/W/centre. Normal map computation is also a stencil (finite differences). |
| **cardinal** | cardinal direction | Exactly N/S/E/W (0°/90°/180°/270°). Cardinal sun direction = shadow rays travel along one row/column = sequential memory access. Diagonal sun = strided rows = cache misses per step. |
| <abbr title="Digital Differential Analyzer">**DDA**</abbr> | Digital Differential Analyzer | Integer ray-stepping algorithm used for shadow sweeps. Steps one grid cell at a time in the sun direction, checking terrain height at each cell. |
| <abbr title="Reorder Buffer">**ROB**</abbr> | Reorder Buffer | Hardware structure tracking all in-flight instructions in an OOO CPU. Determines how far ahead the CPU looks for independent work. M4 ≈ 600 instructions; typical x86 ≈ 350–500. |
| **store buffer** | store buffer | Small hardware buffer (~50–80 entries) holding pending store instructions. Lets the pipeline continue after a store without waiting for cache write. When full → pipeline stalls. |
| **write-allocate** | write-allocate cache policy | On a write miss, fetch the full cache line before writing. Causes every cold write to also generate an RFO (read). Universal on modern CPUs. |
| <abbr title="System on a Chip">**SoC**</abbr> | System on a Chip | CPU, GPU, memory controller, I/O all on one die. M4 Max is a SoC — GPU and CPU share the same 400 GB/s memory bus, no PCIe between them. |
| **GpuScene** | GpuScene (renderer) | Design pattern: all GPU resources allocated once and reused. Only 128 bytes of camera uniforms written per frame. Eliminates per-frame re-upload overhead (~1.37× speedup on M4). |
| **DDR4** | Double Data Rate 4 | Common laptop/desktop RAM 2017–2022. Typical dual-channel bandwidth ~50 GB/s. Win GTX and Asus Pentium use DDR4/DDR3L. |
| **wgpu** | WebGPU Rust implementation | Cross-platform GPU API (Vulkan/Metal/DX12). Used for all compute shaders. wgpu Surface = swap chain presenting frames directly to display without CPU readback. |
| **L1D / L2 / L3** | cache levels | L1D ≈ 32–128 KB, ~4 cycles, private. L2 ≈ 256 KB–4 MB, ~12 cycles, private. L3/LLC ≈ 8–32 MB, ~30–50 cycles, shared. DRAM ≈ 80 ns = ~300 cycles. |
| **stack** | call stack | Compiler-managed memory for local variables and function call frames. Fixed size known at compile time. Allocation = 1 instruction (adjust stack pointer). Automatic lifetime — freed on function return. |
| **heap** | heap memory | Programmer/runtime-managed memory (Vec, Box, malloc). Flexible size, determined at runtime. Persists until explicitly freed. Slower allocation than stack. Can use all available RAM. |
