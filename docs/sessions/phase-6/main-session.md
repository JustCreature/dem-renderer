# Phase 6 Session Log

## Session 1 — 2026-04-06

### Overview

First Phase 6 session. Designed and ran three experiments from the experiment matrix using a
synthetic 5-point stencil kernel (no SIMD complexity, isolates memory access pattern).
Also added valley.png camera render and updated the learning-guide skill with stricter
code-exception enforcement.

---

### Experiment 1: Tile Size Sweep

**Kernel:** 5-point stencil sum, i16 input → f32 row-major output, cold cache.
**File:** `src/benchmarks/phase6.rs` — `bench_tile_size_sweep`

**Results (M4 Max, 3601×3601, ~181 MB/run):**

| Variant | GB/s |
|---|---|
| row-major (baseline) | 60–72 GB/s |
| tiled T=32 | 10.13 GB/s |
| tiled T=64 | 10.76 GB/s |
| tiled T=128 | 11.06 GB/s |
| tiled T=256 | 11.01 GB/s |

**Finding:** Tiled variants are flat across all tile sizes (~11 GB/s) and 6× slower than
row-major. Tile size is irrelevant.

**Root cause confirmed via assembly:** `stencil_rowmajor` auto-vectorised 8-wide NEON
(`ld q0`, `fadd.4s`, `stp q1/q0`, loop stride = 8 pixels). `stencil_tiled` is scalar
(`ldr h0` halfword loads, `fadd s0` scalar) because the `continue` border-skip branch
inside the inner loop prevented auto-vectorisation. The 6× gap = vectorisation width
(~8×) minus prefetcher effects.

**Lesson:** Tiled storage alone doesn't help if the inner loop has control flow that
blocks auto-vectorisation. The compiler's vectoriser is more sensitive to control flow
than to memory layout. Fix would be to hoist the border check out of the inner loop
(9-region split), but this was deemed sufficient for the learning objective — the cause
is proven in assembly.

---

### Experiment 2: Thread Count Scaling (with writes)

**Kernel:** Row-major stencil parallelised via rayon `par_chunks_mut`, cold cache.
**File:** `src/benchmarks/phase6.rs` — `bench_thread_count_scaling`

**Results:**

| Threads | GB/s | Scaling |
|---|---|---|
| 1 | 11.2 | — |
| 2 | 22.7 | 2.0× |
| 4 | 37.9 | 3.4× |
| 6 | 42.9–66.2 | varies |
| 8 | 85.2 | 7.6× |
| 10 | 96.6 | 8.6× |
| 12 | 100.9 | 9.0× |

**Note:** 1-thread rayon = 11 GB/s vs 60–72 GB/s scalar baseline. Gap = rayon closure
not auto-vectorised (same cause as tiled kernel — compiler can't vectorise through the
closure boundary).

**Finding:** Near-linear scaling to ~8 threads, then flattens. Hypothesis: write-path
pressure (store buffer → L1D writeback → L3 → DRAM) is the ceiling, not DRAM bandwidth.

---

### Experiment 3: Thread Count Scaling (read-only, no output write)

**Kernel:** Row-major stencil, sums into per-thread i64 accumulator, no output buffer.
**File:** `src/benchmarks/phase6.rs` — `bench_thread_count_scaling_readonly`

**Results:**

| Threads | GB/s (read-only) | GB/s (with writes) | Ratio |
|---|---|---|---|
| 1 | 34.8 | 11.2 | 3.1× |
| 2 | 64.3 | 22.7 | 2.8× |
| 4 | 120.0 | 37.9 | 3.2× |
| 8 | 226.3 | 85.2 | 2.7× |
| 10 | 259.0 | 96.6 | 2.7× |
| 12 | 246.9 | 100.9 | 2.4× |

**Finding:** Removing the write path gives ~3× more throughput at all thread counts.
Scaling continues linearly past 8 threads (where Exp 2 flattened), confirming that
writes were the bottleneck. Flattening occurs at 10–12 threads at ~259 GB/s — this
is the true DRAM read bandwidth ceiling (~65% of 400 GB/s peak, consistent with
sustained access efficiency).

**Lesson:** The write path (store buffer → writeback → L3 → DRAM) is ~3× narrower
than the read path on M4 Max. For write-heavy kernels, bandwidth saturation occurs
at ~8 cores. For read-only kernels, saturation occurs at ~10 cores at ~260 GB/s.
This asymmetry is a real hardware property — reads benefit from prefetch and fill
buffers; writes must drain through store buffers and write-combining hardware.

---

### Other work this session

- Added `valley.png` render: camera 47°03'52.84"N 11°42'26.24"E, alt 3284m,
  heading 165°, tilt 72°, moved 800m back. Uses `render_gpu_texture` at 8000×2667.
- Updated `skills/learning-guide/SKILL.md`: added ⛔ ABSOLUTE RULE block and
  on-load note to verify CLAUDE.md has the no-silent-code rule at line 1.
- Updated `CLAUDE.md` line 1 with ⛔ enforcement rule.
- Repackaged `learning-guide.skill` → `skills/learning-guide.skill`.
- Added `rayon = "1"` to root `Cargo.toml`.

---

### Open items going into Session 2

- Experiment 4: AoS vs SoA for normal storage (most directly connected to current findings)
- Experiment 5: Morton vs row-major tile layout
- Software prefetch, huge pages, SIMD width, ray packet size — still pending
- GPU shadow via parallel prefix scan — still deferred
- `render_gif` commented out in `main.rs`

---

## Session 2 — 2026-04-06

### Overview

Completed the remaining six experiments from the Phase 6 matrix: AoS vs SoA (Exp 4),
Morton vs row-major (Exp 5), software prefetch (Exp 6), NEON multiple accumulators (Exp 7),
ray packet gather cost (Exp 8), and TLB working set sweep (Exp 9). All experiments implemented
in `src/benchmarks/phase6.rs` and called from `src/main.rs`. Reports generated.

---

### Experiment 4: AoS vs SoA

**Kernel:** dot(normal, sun) and nz-only sum on a 12 Mpix normal map. Single-thread and 10-thread variants.
**File:** `src/benchmarks/phase6.rs` — `bench_aos_vs_soa`

**Results (M4 Max, 12 Mpix, AoS=155 MB / SoA=3×51 MB):**

| Kernel | SoA GB/s | AoS GB/s | Ratio |
|---|---|---|---|
| dot product (12 B/pix) | 22.4–24.2 | 22.4–24.1 | 1.00× |
| nz-only, 1 thread | 8.0–8.3 | 8.1–8.4 | 1.00× |
| nz-only, 10 threads | 66.0 | 58.3 logical (175 actual) | 1.13× |

**Finding:** Single-thread is tied because the serial `fadd` dep chain (3-cycle latency) is the
bottleneck — compute-bound, not memory-bound. With 10 threads the bottleneck shifts to memory,
but the M4 Max's 400 GB/s bus has enough headroom to serve AoS at 175 GB/s without saturation.
SoA wins only 1.13× instead of the expected 3× because bandwidth is not saturated.

**Lesson:** SoA's bandwidth advantage only materialises when (a) the kernel is genuinely
memory-bound, not dep-chain compute-bound, and (b) the memory subsystem is near saturation.
On a wide-bus machine, both conditions are rarely met simultaneously at single-thread scale.

---

### Experiment 5: Morton vs Row-major Tile Ordering

**Kernel:** 5-point stencil on a Morton-indexed heightmap vs row-major tiled, T=64, cold cache.
**File:** `src/benchmarks/phase6.rs` — `bench_morton_vs_rowmajor`

**Results:**

| Variant | GB/s |
|---|---|
| row-major (vectorised baseline) | 64–68 |
| tiled row-major T=64 | 10.25–10.40 |
| tiled Morton T=64 | 10.43 (1.00×) |

**Finding:** Morton reduces N/S tile distance from ~456 KB to 8–32 KB (L2 → L1 range).
Zero measurable effect. The OOO ROB (~600 entries) can see ~60 loop iterations ahead,
enough to absorb the 12-cycle L2 latency inside compute time. The scalar loop is the floor.

**Lesson:** Morton ordering helps only when the access pattern is compute-light and the
loop executes faster than the hardware can hide cache latency. When the scalar loop itself
is the bottleneck, cache distance improvements are hidden for free by OOO execution.

---

### Experiment 6: Software Prefetch

**Kernel:** 1M random reads into a 256 MB f32 array using pre-shuffled indices. Variants
with `prfm pldl1keep` at distances D=4, 16, 64.
**File:** `src/benchmarks/phase6.rs` — `bench_software_prefetch`

**Results:**

| Variant | GB/s | vs baseline |
|---|---|---|
| no prefetch | 1.20 | — |
| prfm D=4 | 1.28 | +7% |
| prfm D=16 | 1.31 | +9% |
| prfm D=64 | 1.37 | +14% |

**Finding:** Only 14% improvement at D=64. The M4 Max's ~600-instruction ROB already issues
speculative loads 60+ iterations ahead for simple loop patterns. Explicit `prfm` is largely
redundant — the hardware is already doing the same thing.

**Lesson:** Software prefetch provides large gains on in-order or small-ROB processors.
On an aggressive OOO superscalar, it's mostly redundant for simple loop access patterns.
It only helps where the OOO window can't see the address: pointer-chasing, dependent indices.

---

### Experiment 7: NEON Multiple Accumulators

**Kernel:** SoA dot product (normal · sun) with explicit NEON via `#[target_feature(enable = "neon")]`.
1, 4, and 8 independent accumulator registers.
**File:** `src/benchmarks/phase6.rs` — `bench_neon_accumulators`

**Results (12 Mpix, 155 MB total):**

| Variant | GB/s | Bottleneck |
|---|---|---|
| auto-vec scalar | 23.7 | compute — fadd dep chain, 3-cycle lat |
| NEON explicit 1 acc | 18.0 | compute — vfmaq dep chain, 4–5 cycle lat |
| NEON explicit 4 acc | 71.0 | DRAM — both FP ports busy |
| NEON explicit 8 acc | 120.0 | SLC — data partially in SLC |

**Surprise:** NEON 1-acc is slower than auto-vec scalar. `vfmaq_f32` latency is 4–5 cycles
vs scalar `fadd` 3 cycles. With a serial dep chain, higher instruction latency = lower throughput
regardless of lane width.

**Transition model:**
- 1 acc → compute-bound (fadd latency ceiling)
- 4 acc → DRAM-bound (~65 GB/s, M4 Max's 2 FP ports both busy, memory is the limit)
- 8 acc → SLC-bound (~120 GB/s, eviction insufficient to flush all 155 MB, data hits SLC)

**Lesson:** Breaking the serial dep chain is the most important SIMD optimisation. Multiple
accumulators unlock the FP port throughput that a single accumulator leaves idle.

---

### Experiment 8: Ray Packet Gather Cost

**Kernel:** Simulated 4-wide ray packet gathers on a real heightmap. Variants: 1-wide scalar,
4-wide adjacent columns, 4-wide strided rows, 4-wide fully random.
**File:** `src/benchmarks/phase6.rs` — `bench_gather_ray_packets`

**Results (2M positions, 3601×3601, 25 MB heightmap):**

| Variant | Total time | ns/step | ns/pixel |
|---|---|---|---|
| 1-wide scalar | 3.4 ms | 1.70 ns | 1.70 ns |
| 4-wide adjacent cols | 4.6 ms | 2.30 ns | 0.57 ns |
| 4-wide strided rows | 6.1 ms | 3.06 ns | 0.76 ns |
| 4-wide fully random | 2.1 ms | 4.1 ns | 1.03 ns |

**Finding:** Adjacent-column gathers are 3× faster per pixel (4 values in 1 cache line).
Strided-row gathers are 2.2× faster per pixel despite 4 cache misses — OOO issues all 4 in
parallel. Fully random gathers are 1.65× faster per pixel due to better memory-level parallelism
than the serial 1-wide reduction chain.

**Note on reporting error:** the 4-wide random benchmark reported 8.35 GB/s using 4× the actual
bytes accessed. Correct per-access rate is ~1.9 GB/s. Use ns/pixel as the reliable metric.

**Lesson:** Phase 4 NEON = scalar (1.00×) because: (1) rays diverge after ~50 steps converting
adjacent access to random, (2) the height compare + ray advance compute doesn't vectorise cleanly
across diverged rays, (3) the compiler auto-vectorises scalar anyway.

---

### Experiment 9: TLB Working Set Sweep

**Kernel:** 1M random reads into arrays of increasing size (16 KB → 256 MB), pre-shuffled indices.
**File:** `src/benchmarks/phase6.rs` — `bench_tlb_sweep`

**Results (macOS/M4 Max, 16KB base pages):**

| Size | 16KB pages | GB/s | Layer |
|---|---|---|---|
| 16 KB | 1 | 8.20 | L1 cache + L1 DTLB |
| 256 KB | 16 | 8.20 | L3/SLC + L1 DTLB |
| 1 MB | 64 | 7.51 | L3/SLC + L1 DTLB |
| 4 MB | 256 | 6.40 | ← L1 DTLB full |
| 16 MB | 1024 | 3.77 | ← L2 TLB pressure |
| 64 MB | 4096 | 1.91 | L2 TLB miss + page walks |
| 256 MB | 16384 | 1.38 | page walks + DRAM latency |

**Findings:** First knee at 4 MB confirms L1 DTLB ~256 entries × 16KB. Second knee at 16–64 MB
confirms L2 TLB capacity ~1000–2000 entries × 16KB ≈ 16–32 MB. macOS 16KB pages give 4× more
TLB coverage than x86 4KB pages. The 26 MB heightmap (1625 pages) fits in L2 TLB — no page walks
during renderer operation. The 256 MB Exp 6 array (16384 pages) spills to page walks.

**Lesson:** TLB coverage determines whether random access pays DRAM latency alone or DRAM +
page-walk latency. macOS's 16KB pages push both TLB knees 4× further out vs x86 Linux.

---

### Other work this session

- Generated `docs/lessons/phase-6/long-report.md` — comprehensive 9-experiment textbook
- Generated `docs/lessons/phase-6/short-report.md` — reference card with all tables

---

### Open items carried to Phase 7

- GPU shadow via parallel prefix scan (deferred from Phase 5)
- `render_gif` commented out in `main.rs`
