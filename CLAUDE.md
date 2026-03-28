# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Interaction Mode

- **Guide, don't implement.** The user is building this to learn. Explain *why* something works at the hardware level, point to the right direction, suggest experiments — but do not write code or execute commands unless explicitly asked.
- **Assume strong technical curiosity.** The user wants full-depth explanations: cache-line math, TLB reach calculations, ROB/store-buffer reasoning, branch predictor behavior. Don't simplify unless asked.
- **Encourage measurement over intuition.** When the user asks "which is faster?", the answer is almost always "profile it — here's how and what counters to look at."
- **Build layered mental models.** Start from the hardware constraint (cache size, SIMD width, pipeline depth), derive the software implication, then suggest the experiment to validate.
- **Go full hardware depth.** Reason about store buffers, ROB size, retirement rate, branch predictor internals (TAGE), TLB pressure, prefetcher training, port pressure — not just "use SIMD and cache lines."

## Project Purpose

A learning-first, cache-optimized terrain + sunlight renderer in Rust using real USGS DEM data (~4000×4000, ~32–64 MB). The explicit goal is deep hardware understanding — memory hierarchy, SIMD utilization, TLB behavior, store buffers, ROB limits, branch predictor internals — not just producing a working renderer. Every design decision must be justified at the microarchitectural level, and every optimization must be validated with measured numbers.

## Status

**Current phase: Phase 3** (Phases 0, 1, 2 complete)

Phase 0 artifacts:
- `crates/profiling/src/lib.rs` — `now()` (cntvct_el0 via inline asm), `timed()`, tests
- `src/main.rs` — scalar and NEON SIMD benchmarks for seq_read, random_read, seq_write, random_write
- `docs/lessons/phase-0/long-report.md` — comprehensive Phase 0 student textbook
- `docs/lessons/phase-0/short-report.md` — comprehensive Phase 0 reference
- `docs/sessions/phase-0/main-session.md` — session log

Phase 0 baseline numbers (M4 Max, 256 MB):
- seq_read scalar: 5.7–6.7 GB/s | seq_read SIMD: 21.8–37 GB/s
- random_read scalar: 0.6 GB/s | random_read SIMD: 1.4 GB/s
- Sequential/random ratio: 11–16× — this number drives all Phase 1+ tiling decisions

Phase 1 artifacts:
- `crates/dem_io/src/heightmap.rs` — `Heightmap`, `parse_bil`, `fill_nodata`, `parse_hdr`
- `crates/dem_io/src/tiled.rs` — `TiledHeightmap`, `from_heightmap(&Heightmap, tile_size)`, `get()` with `#[inline(always)]`
- `crates/dem_io/src/aligned.rs` — `AlignedBuffer`: page-aligned (4096-byte) manual allocation with `Drop`, `Deref`, `DerefMut`, `unsafe impl Send + Sync`
- `crates/dem_io/src/lib.rs` — module declarations, re-exports
- `src/main.rs` — neighbour-sum benchmarks (row-major, tiled row-major, tiled tile-order), cold-cache eviction pattern
- `docs/lessons/phase-1/build_heightmap/` — reports for DEM parsing stage
- `docs/lessons/phase-1/build_tiled_heightmap/` — reports for tiled layout + aligned allocation stage
- `docs/sessions/phase-1/` — session logs

Phase 1 key numbers (M4 Max, cold cache, tile_size=128):
- row-major neighbour sum: 26–46 GB/s (prefetcher detects stride-3601)
- tiled row-major iteration: 3.7–4.0 GB/s (iteration order mismatch + `get()` overhead)
- tiled tile-order iteration: 3.0 GB/s (`get()` decomposition overhead dominates)
- Lesson: `get()` abstraction cannot demonstrate tiling benefit — Phase 2 must use direct tile pointer arithmetic

Phase 2 artifacts:
- `crates/terrain/src/lib.rs` — `NormalMap` (SoA), `SendPtr`, module declarations
- `crates/terrain/src/row_major.rs` — `compute_normals_scalar`, `compute_normals_neon`, `compute_normals_neon_8`, `compute_normals_neon_parallel`
- `crates/terrain/src/tiled.rs` — `compute_normals_neon_tiled`, `compute_normals_neon_tiled_parallel`
- `src/main.rs` — normal map benchmark functions and PNG output
- `docs/lessons/phase-2/long-report.md` — comprehensive Phase 2 student textbook
- `docs/lessons/phase-2/short-report.md` — comprehensive Phase 2 reference
- `docs/sessions/phase-2/main-session.md` — session log

Phase 2 key numbers (M4 Max, cold cache, isolated runs):
- Scalar (black_box): 8.1 GB/s | Auto-vectorized scalar: 24.1 GB/s | NEON 4-wide: 28.8 GB/s
- NEON parallel (10 cores): 42–50 GB/s cold | ~117 GB/s warm
- Tiled NEON single: 22.9 GB/s | Tiled NEON parallel: 34.0 GB/s (worse than row-major — output is row-major, writes dominate)
- Lesson: tiling helps input reads but hurts output writes when output layout doesn't match iteration order

Known open items from Phase 2:
- `profiling::timed(label, ...)` in `random_read`, `seq_write`, `random_write` uses wrong label `"seq_read"` — fix
- `fill_nodata` division-by-zero if all 4 directions hit boundary without finding valid data
- Drop `bil_bytes` early in `parse_bil` to halve peak memory
- Tiled normal computation leaves cross-tile boundary pixel rows as zero (incorrect) — halo exchange needed to fix
- Phase 3 shadow sweep will also be write-heavy — same DRAM bandwidth ceiling applies

Implementation follows the phased plan in `docs/planning/global_plan.md`.

## Build Commands

Once the workspace is scaffolded:

```sh
cargo build --release
cargo bench -p terrain                        # Benchmark only the terrain crate
cargo bench -p render_cpu
cargo build --workspace --exclude render_gpu  # Skip heavy GPU crate during CPU work
RUSTFLAGS="-C target-cpu=native" cargo build --release  # Enable AVX2/NEON
```

**Build profiles** (to add to workspace `Cargo.toml`):
```toml
[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 1

[profile.bench]
inherits = "release"
debug = true  # symbols for perf report / Instruments
```

Use `#[inline(never)]` during profiling so functions appear as distinct symbols in `perf report`. Switch to `#[inline]` + LTO for final benchmark numbers.

## Architecture

### Cargo Workspace Structure

```
dem_renderer/
├── Cargo.toml          # workspace root
├── src/main.rs         # binary: CLI entry, orchestrates all phases
├── crates/
│   ├── dem_io/         # .hgt parsing, tile stitching, aligned allocation
│   ├── terrain/        # normals, shadow sweep, Morton tiling
│   ├── render_cpu/     # CPU raymarcher, SIMD packet tracing, shading
│   ├── render_gpu/     # wgpu compute pipeline, WGSL shaders
│   └── profiling/      # rdtsc/cntvct cycle counters, CSV timing
├── benches/            # Criterion benchmarks (normals.rs, shadows.rs, raymarcher.rs)
├── scripts/            # perf_stat.sh, instruments.sh
├── data/               # .hgt files (gitignored, ~32–64 MB each)
└── docs/
    ├── planning/global_plan.md
    └── learnings/project_structure.md
```

### Dependency DAG

```
profiling (leaf)
    ↑
dem_io
    ↑
terrain
    ↑         ↑
render_cpu   render_gpu
    ↑         ↑
  main.rs
```

Types are defined in the crate that produces them (`Heightmap` in `dem_io`, `NormalMap`/`ShadowMask` in `terrain`) — no shared "god types" crate.

### Crate Responsibilities

- **`dem_io`**: Parse USGS SRTM `.hgt` files (flat big-endian `i16`, 1201×1201 or 3601×3601 per tile). Owns the memory layout decision: tiled storage (64²–256² tiles), Z-order (Morton curve) vs tile-linear ordering, 64-byte and 4096-byte aligned allocation.
- **`terrain`**: Surface normals via finite differences, SoA layout (`Vec<f32>` for nx, ny, nz separately). Sun shadow sweep (O(N²) DDA-based horizon-angle propagation). SIMD: `_mm256_max_ps` / `vmaxq_f32` for the running-max.
- **`render_cpu`**: Pinhole camera raymarcher, packet raytracing (8 lanes AVX2 / 4 lanes NEON), screen-space tiled ray dispatch, `rayon` parallelism.
- **`render_gpu`**: wgpu compute pipeline, WGSL shader per-pixel raymarching. Heightmap as 2D texture vs storage buffer (both variants for comparison). Workgroup size 8×8.
- **`profiling`**: `rdtsc` (x86) / `cntvct_el0` (AArch64) wrappers, CSV timing emitter, `perf stat` invocation helpers.

## Key Design Decisions

| Decision | Rationale |
|---|---|
| SoA over AoS for normals | Load 8 consecutive nx values in one AVX2 instruction |
| Tiled memory layout | Working set fits in L1/L2; spatial memory locality matches spatial data locality |
| 64-byte alignment (cache-line), 4096-byte (page) | Prefetcher friendliness, avoid cache-line splits |
| Branchless inner loops | SIMD masks / `cmov` in hot paths (shadow sweep, ray termination) |
| `#[inline(never)]` on profiled functions | Appear as distinct symbols in `perf report` / Instruments |
| `codegen-units = 1` | Avoid cross-function optimization loss from multi-unit compilation |
| Thin LTO in release | Cross-crate inlining for hot paths without full LTO compile time |

## Coding Conventions

- Language is Rust — stable where possible, nightly for `std::simd` / `core::arch` features not yet stabilized.
- `unsafe` only for SIMD intrinsics and aligned allocation — document the safety invariant inline.
- Prefer `core::arch` over `std::simd` when stable intrinsics cover the operation.
- Name SIMD dispatch functions explicitly: `compute_normals_avx2()`, `compute_normals_neon()`, `compute_normals()` (dispatcher).
- Benchmarks in `benches/` using Criterion. Micro-benchmarks for individual kernels, macro-benchmarks for full frames.

## Profiling

**Target hardware**: Apple Silicon (NEON 128-bit, 128 KB L1D on perf cores) and x86-64 (AVX2/AVX-512, 32–48 KB L1D).

Before claiming an optimization works, measure:
- Wall-clock (Criterion or manual)
- `perf stat`: cycles, instructions, IPC
- L1/L2/L3 miss rates, dTLB miss rate, branch misprediction rate
- Apple Silicon: Instruments CPU Counters template

Key counters: `cache-misses`, `L1-dcache-load-misses`, `dTLB-load-misses`, `instructions`, `cycles`, `branches`, `branch-misses`, `resource_stalls.sb` (store buffer stalls), `fp_ret_sse_avx_ops.all`.

## Custom Procedures

### GENERATE_REPORTS{}

When the user types `GENERATE_REPORTS{}`, do the following — no questions, no confirmation:

1. **Determine the current phase** from the Status section above (e.g., "Phase 0").

2. **Read the session log** at `docs/sessions/phase-N/main-session.md` to reconstruct what was covered.

3. **Read the existing reports** at `docs/lessons/phase-N/long-report.md` and `docs/lessons/phase-N/short-report.md` if they exist.

4. **Write `docs/lessons/phase-N/long-report.md`** — a comprehensive student textbook covering every concept from the phase. Structure: numbered parts, every term defined on first use, all hardware reasoning included, all code patterns shown with explanation, all gotchas and errors documented, all benchmark results with analysis. Think: a student who missed the session can learn everything from this document alone.

5. **Write `docs/lessons/phase-N/short-report.md`** — a comprehensive reference document. Structure: numbered sections, every topic covered with brief but self-contained explanations (2–6 sentences per concept), all code patterns included, all tables and numbers. Think: a student who did the session uses this to refresh their full mental model in 10–15 minutes.

6. **Do not update the session log** — that is written during or immediately after the session, not on demand.

The long report is a textbook. The short report is a thorough reference, not a cheatsheet.

---

## Implementation Phases

See `docs/planning/global_plan.md` for full details:

- **Phase 0**: Cargo workspace, profiling harness, baseline memory bandwidth numbers
- **Phase 1**: DEM ingestion, tiled memory layout, aligned allocation
- **Phase 2**: Normal computation — SIMD finite differences, SoA, rayon
- **Phase 3**: Sun shadow sweep — SIMD running-max, branchless vs branchy comparison
- **Phase 4**: CPU raymarcher — packet tracing, screen-space tiling, divergence
- **Phase 5**: GPU renderer — wgpu compute, texture vs buffer comparison, occupancy
- **Phase 6**: Experiment matrix (AoS vs SoA, tile sizes, Morton vs row-major, huge pages, SIMD width, thread count, ray packet size)
- **Phase 7**: Stretch goals (out-of-core `mmap`, ambient occlusion, animated sun)
