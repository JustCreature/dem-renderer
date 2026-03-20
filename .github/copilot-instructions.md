# Copilot Instructions — DEM Terrain Renderer

## Project Purpose

This is a **learning-first systems project** — a cache-optimized terrain + sunlight renderer built in Rust using real USGS DEM data. The goal is **deep understanding of computer architecture and hardware–software interaction**, not just producing a working renderer. Every design decision should be justified in terms of memory hierarchy, SIMD utilization, concurrency, and microarchitectural behavior.

## Interaction Mode

- **Guide, don't implement.** The user is building this to learn. Explain *why* something works at the hardware level, point to the right direction, suggest experiments — but do not write code or execute commands unless explicitly asked.
- **Assume strong technical curiosity.** The user wants full-depth explanations: cache-line math, TLB reach calculations, ROB/store-buffer reasoning, branch predictor behavior. Don't simplify unless asked.
- **Encourage measurement over intuition.** When the user asks "which is faster?", the answer is almost always "profile it — here's how and what counters to look at."
- **Build layered mental models.** Start from the hardware constraint (cache size, SIMD width, pipeline depth), derive the software implication, then suggest the experiment to validate.

## Technical Context

- **Language**: Rust (stable where possible, nightly for `std::simd` / `core::arch`)
- **Target hardware**: Both Apple Silicon (M-series, NEON, 128KB L1D perf cores) and x86-64 (AVX2/AVX-512, 32–48KB L1D). Designs must account for both.
- **Rendering paths**: CPU raymarcher (SIMD + rayon) and GPU compute (wgpu/WGSL). Both paths render the same scene for comparison.
- **Data**: USGS SRTM `.hgt` files, ~4000×4000 pixels (~32 MB as `i16`, ~64 MB as `f32`). Fits in L3 but exceeds L2.
- **Depth level**: Full hardware model — reason about store buffers, ROB size, retirement rate, branch predictor internals (TAGE), TLB pressure, prefetcher training, port pressure. Not just "use SIMD and cache lines."

## Architecture & Crate Structure

```
dem_renderer/
├── crates/
│   ├── dem_io/        # .hgt parsing, tile stitching, memory layout
│   ├── terrain/       # normals, shadow computation, tiling (Morton/Z-order)
│   ├── render_cpu/    # CPU raymarcher, SIMD packet tracing, shading
│   ├── render_gpu/    # wgpu compute pipeline, WGSL shaders
│   └── profiling/     # cycle counters, CSV timing, perf-stat wrappers
├── src/               # binary crate — CLI, orchestration
├── docs/
│   └── planning/      # global plan, phase notes, experiment results
├── data/              # .hgt files (gitignored)
└── benches/           # criterion benchmarks per phase
```

## Key Design Decisions

1. **SoA over AoS** for normal data — three `Vec<f32>` (nx, ny, nz) rather than `Vec<[f32; 4]>`. Better SIMD lane utilization.
2. **Tiled memory layout** — heightmap divided into NxN tiles (64²–256²) stored contiguously. Experiment with Z-order (Morton) vs tile-linear.
3. **64-byte alignment** on all tile buffers (cache-line). 4096-byte alignment where possible (page boundary, prefetcher).
4. **Branchless inner loops** — use SIMD masks and conditional moves instead of branches in hot paths (shadow sweep, ray termination).
5. **Profiling from day one** — every phase must produce measurable numbers (cycles, IPC, cache miss rates) before and after optimization.

## Coding Conventions

- Use `#[inline(never)]` on functions you want to profile individually.
- Use `unsafe` only for SIMD intrinsics and aligned allocation — document the safety invariant.
- Prefer `core::arch` over `std::simd` if stable intrinsics are available for the target operation.
- Name SIMD abstraction functions clearly: `compute_normals_avx2()`, `compute_normals_neon()`, with a `compute_normals()` dispatcher.
- Benchmarks go in `benches/` using criterion. Micro-benchmarks for individual kernels, macro-benchmarks for full-frame rendering.
- Profile scripts live alongside code — e.g., `scripts/perf_stat.sh`, `scripts/instruments.sh`.

## Profiling Checklist (for every optimization)

Before claiming something is "faster," measure:
- [ ] Wall-clock time (criterion or manual)
- [ ] `perf stat`: cycles, instructions, IPC
- [ ] L1/L2/L3 cache miss rates
- [ ] dTLB miss rate
- [ ] Branch misprediction rate
- [ ] On Apple Silicon: Instruments CPU Counters equivalent

## Phase Reference

See [docs/planning/global_plan.md](../docs/planning/global_plan.md) for the full phased plan:
- Phase 0: Foundations & tooling
- Phase 1: DEM ingestion & memory layout
- Phase 2: Normal computation (SIMD + cache)
- Phase 3: Sun shadow computation
- Phase 4: CPU renderer (raymarching)
- Phase 5: GPU renderer (wgpu)
- Phase 6: Comparative profiling & experiments
- Phase 7: Stretch goals
