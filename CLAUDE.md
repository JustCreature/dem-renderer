# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Learning Guide

This project uses the `learning-guide` skill (located at `skills/learning-guide/SKILL.md`). It is **always active**.

### Commands

| Command | What it does |
|---|---|
| `--R` | Generate / update `docs/lessons/phase-N/long-report.md` and `short-report.md` |
| `--\|` | Save session to `docs/sessions/phase-N/main-session.md` and update CLAUDE.md |
| `--\|--` | Restore from the most recent session file in `docs/sessions/` |
| `--\|--path` | Restore from a specific file, e.g. `--\|--docs/sessions/phase-2/session-1.md` |
| `--s` | Show current phase, completion status, open items, last session summary |
| `--v` | Finalise current phase if all planned items are complete |
| `--v--FORCE` | Finalise unconditionally; carry incomplete items as open items to next phase |

---

## Interaction Mode

- **Guide, don't implement.** The user is building this to learn. Explain *why* something works at the hardware level, point to the right direction, suggest experiments — but do not write code or execute commands unless explicitly asked.
- **Assume strong technical curiosity.** The user wants full-depth explanations: cache-line math, TLB reach calculations, ROB/store-buffer reasoning, branch predictor behavior. Don't simplify unless asked.
- **Encourage measurement over intuition.** When the user asks "which is faster?", the answer is almost always "profile it — here's how and what counters to look at."
- **Build layered mental models.** Start from the hardware constraint (cache size, SIMD width, pipeline depth), derive the software implication, then suggest the experiment to validate.
- **Go full hardware depth.** Reason about store buffers, ROB size, retirement rate, branch predictor internals (TAGE), TLB pressure, prefetcher training, port pressure — not just "use SIMD and cache lines."

## Project Purpose

A learning-first, cache-optimized terrain + sunlight renderer in Rust using real USGS DEM data (~4000×4000, ~32–64 MB). The explicit goal is deep hardware understanding — memory hierarchy, SIMD utilization, TLB behavior, store buffers, ROB limits, branch predictor internals — not just producing a working renderer. Every design decision must be justified at the microarchitectural level, and every optimization must be validated with measured numbers.

## Status

**Current phase: Phase 6** (Phases 0, 1, 2, 3, 4, 5 complete)

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

Phase 3 artifacts:
- `crates/terrain/src/shadow.rs` — `DdaSetup`, `dda_setup()`, `ShadowMask`, `compute_shadow_scalar`, `compute_shadow_scalar_branchless`, `compute_shadow_scalar_with_azimuth`, `compute_shadow_neon`, `compute_shadow_neon_parallel`, `compute_shadow_neon_parallel_with_azimuth`
- `crates/terrain/src/lib.rs` — exports updated for all shadow functions
- `src/main.rs` — benchmark functions for all shadow variants, `create_rgb_png`
- `docs/lessons/phase-3/long-report.md` — comprehensive Phase 3 student textbook
- `docs/lessons/phase-3/short-report.md` — comprehensive Phase 3 reference
- `docs/sessions/phase-3/main-session.md` — session log

Phase 3 key numbers (M4 Max, cold cache, isolated runs):
- Scalar branchy: 8.1 GB/s | Scalar branchless: 10.6 GB/s | NEON 4-wide: 17.4 GB/s | NEON parallel (10 cores): 58.6 GB/s
- Bottleneck arc: latency-bound (64%) → latency+memory → purely memory-bound
- Lesson: branchless wins (31%) despite accurate branch prediction — unconditional store pattern is more pipeline-friendly
- Lesson: NEON vectorises across rows (not within), breaking the serial dependency chain 4×
- NEON parallel gives 3.4× from parallelism (not 10×) — memory bandwidth is the ceiling at 58.6 GB/s

Phase 4 artifacts:
- `crates/render_cpu/src/camera.rs` — `Camera`, `Ray`, `Camera::new`, `ray_for_pixel`
- `crates/render_cpu/src/vector_utils.rs` — `pub(crate)`: `add`, `sub`, `scale`, `normalize`, `cross`
- `crates/render_cpu/src/raymarch.rs` — `raymarch()`, `binary_search_hit()` (private)
- `crates/render_cpu/src/raymarch_neon.rs` — `RayPacket` (SoA), `raymarch_neon()`, `binary_search_hit_neon()` (private)
- `crates/render_cpu/src/render.rs` — `render()` (scalar), `render_par()` (rayon), `shade()` (pub(crate))
- `crates/render_cpu/src/render_neon.rs` — `render_neon()`, `render_neon_par()` (NEON + rayon)
- `crates/render_cpu/src/lib.rs` — mod declarations, pub re-exports
- `src/frame_render_cpu.rs` — camera setup from Google Earth coords, all 4 render variants timed
- `docs/lessons/phase-4/long-report.md` — comprehensive Phase 4 student textbook
- `docs/lessons/phase-4/short-report.md` — comprehensive Phase 4 reference
- `docs/sessions/phase-4/main-session.md` — session log

Phase 4 key numbers (M4 Max, 2000×900 image, step_m = dx_meters ≈ 20.7m):
- Scalar single-thread: 0.80s | NEON single-thread: 0.80s (same — gather overhead cancels SIMD gain)
- Scalar parallel (10 cores): 0.08s | NEON parallel (10 cores): 0.08s — 10× speedup, near-ideal scaling
- Average steps per ray: 506 (≈10.5 km travel)
- Effective read rate: ~22 GB/s << M4 Max 400 GB/s — not bandwidth-limited, compute-bound
- Lesson: for memory-bound code with sequential access, parallelism >> manual SIMD; NEON gain cancelled by gather overhead + compiler auto-vectorization of scalar
- Lesson: screen-space tiling not beneficial here — horizontal 1×4 packets already give optimal cache-line reuse; bottleneck is gather count not cache misses

Phase 5 artifacts:
- `crates/render_gpu/src/context.rs` — `GpuContext { device, queue }`, `new()` does one-time Instance→Adapter→Device init (~80ms)
- `crates/render_gpu/src/render_buffer.rs` — `render_gpu_buffer()`: heightmap as storage buffer, normals+shadow uploaded each call
- `crates/render_gpu/src/render_rexture.rs` — `render_gpu_texture()`: heightmap as 2D texture + sampler
- `crates/render_gpu/src/render_gpu_combined.rs` — `render_gpu_combined()`: normals computed on GPU, no normal map upload
- `crates/render_gpu/src/normals_gpu.rs` — `compute_normals_gpu()`: wgpu compute pass, outputs NormalMap
- `crates/render_gpu/src/shadow_gpu.rs` — `compute_shadow_gpu()`: GPU shadow sweep (serial, slower than CPU NEON)
- `crates/render_gpu/src/scene.rs` — `GpuScene`: all static GPU resources persistent; only 128 bytes written per frame
- `crates/render_gpu/src/camera.rs` — `CameraUniforms` (128 bytes, std140-padded), build_camera_uniforms()
- `crates/render_gpu/src/shader_buffer.wgsl` — raymarching shader using storage buffer for heightmap
- `crates/render_gpu/src/shader_texture.wgsl` — raymarching shader using 2D texture for heightmap
- `crates/render_gpu/src/shader_normals.wgsl` — normals compute pass (finite differences on GPU)
- `crates/render_gpu/src/shader_shadow.wgsl` — shadow sweep compute pass (GPU)
- `src/benchmarks/multi_frame.rs` — 4-way multi-frame benchmark: CPU parallel, GPU separate, GPU combined, GPU scene
- `src/render_gif.rs` — 60-frame GIF renderer at 1600×533, 20fps, using GpuScene
- `src/frame_render_final.rs` — camera setup from Google Earth coords, CPU + GPU single-image renders
- `docs/lessons/phase-5/long-report.md` — comprehensive Phase 5 student textbook
- `docs/lessons/phase-5/short-report.md` — comprehensive Phase 5 reference
- `docs/sessions/phase-5/benchmark_results.md` — full benchmark table from 2026-04-06 run
- `docs/sessions/phase-5/main-session.md` — session log

Phase 5 key numbers (M4 Max, 8000×2667 = 21.3 Mpix, step_m = dx/1.0 ≈ 20.7m):
- GPU buffer: 130 ms | GPU texture: 170 ms | GPU combined: 90 ms | CPU parallel: 1260 ms
- Buffer beats texture: 1.3× (stripe-like ray access doesn't benefit 2D texture cache; sampler unit adds latency)
- GPU combined vs buffer: combined skips 156 MB normal upload; wins by 1.4×
- Multi-frame per-frame: CPU 1730ms | GPU separate 133ms | GPU combined 120ms | GPU scene 98ms
- GpuScene speedup over CPU: 17.7× — only 128 bytes written per frame; 85 MB readback is the hard floor (~88ms)
- Shadow CPU vs GPU: NEON parallel 1.5ms vs GPU 26ms — CPU wins 17× (serial running-max dependency)
- Diagonal shadow 2.4× slower than cardinal (strided cache-line access)
- Workgroup size (all variants 64–256 threads, shapes 8×8 to 32×8): all within ±3% (~136–140ms) — readback dominates, compute dispatch is ~5–10ms

Phase 5 lessons:
- GPU wins rendering (17.7×) because raymarching is embarrassingly parallel — no inter-pixel dependencies
- CPU wins shadow because running-max is serial per row; GPU shader cores run at lower clock with no advantage
- wgpu bind groups store GPU addresses, not CPU-side Arc refs — all referenced resources must be kept alive in the owning struct
- `write_buffer` updates buffer contents in-place; bound bind group sees new data automatically on next dispatch
- Normals computed once on GPU in `GpuScene::new()` and never read back; shadow computed on CPU (NEON) and uploaded
- The GPU readback floor (~88ms for 85 MB) limits max frame rate; eliminating it requires a display/swap-chain architecture
- Workgroup size (shape or thread count 64→256) has no effect: all variants ±3% because the 85 MB readback dominates; dispatch itself is ~5–10 ms

Known open items from Phase 5:
- GPU shadow via parallel prefix scan not implemented — would potentially match CPU NEON for cardinal direction
- Occupancy analysis via Instruments/Metal GPU trace deferred — requires full Xcode.app; not a priority since workgroup tuning shows no effect with readback dominating
- GIF rendering uses CPU readback + GIF encoding; the 85 MB readback dominates per-frame cost
- `render_gif::render_gif` is commented out in main.rs — re-enable when generating animations

Known open items from Phase 4:
- Supersampled ray optimization considered but not implemented: march 1 reference ray, approximate 3 neighbor heights via `h ≈ h_center + grad_x * Δcol + grad_y * Δrow` (using Phase 2 normal map). Would reduce gather 4→1 per step. Breaks at sharp discrete peaks.

Known open items from Phase 3:
- `compute_shadow_neon_parallel_with_azimuth` benchmarked at sunset (270°): 26.3 GB/s vs cardinal 55.4 GB/s — 2.1× gap confirmed
- `profiling::timed(label, ...)` in `random_read`, `seq_write`, `random_write` uses wrong label `"seq_read"` — fix
- `fill_nodata` division-by-zero if all 4 directions hit boundary without finding valid data
- Drop `bil_bytes` early in `parse_bil` to halve peak memory
- Tiled normal computation leaves cross-tile boundary pixel rows as zero (incorrect) — halo exchange needed to fix

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

### Reports — use `--R`

`--R` replaces `GENERATE_REPORTS{}`. See `learning-guide/references/reporting.md` for full spec.

No confirmation needed — run immediately when `--R` is typed. Read the current phase's session log
and any existing reports, then write or fully update both:
- `docs/lessons/phase-N/long-report.md` — comprehensive student textbook
- `docs/lessons/phase-N/short-report.md` — thorough reference (refresh in 10–15 min)

Do not update the session log during `--R` — that is `--|`'s job.

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
