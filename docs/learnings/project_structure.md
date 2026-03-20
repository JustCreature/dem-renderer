# Project Structure in Rust — DEM Terrain Renderer

## Cargo Workspace Layout

Rust's workspace system lets you have multiple crates (libraries + binaries) that share a single `Cargo.lock` and `target/` directory. This matters for us because:

1. **Isolated benchmarking** — you can `cargo bench -p terrain` without compiling the GPU crate. This means you measure *just* the kernel you care about, no noise from unrelated code.
2. **Compilation units** — each crate is a separate codegen unit. The compiler can optimize within a crate aggressively (inlining, LTO). Across crates, you need LTO enabled or `#[inline]` hints. This is a real performance consideration.
3. **Feature gating** — the GPU crate pulls in `wgpu` (heavy dependency). You don't want that compiling when you're iterating on CPU SIMD kernels.

Here's the structure with the reasoning:

```
dem_renderer/
├── Cargo.toml              # [workspace] — defines members, shared dependencies
├── src/
│   └── main.rs             # Binary crate — CLI entry point, orchestrates everything
├── crates/
│   ├── dem_io/
│   │   ├── Cargo.toml      # No dependencies beyond std (maybe byteorder)
│   │   └── src/lib.rs      # .hgt parsing, tile stitching, aligned allocation
│   ├── terrain/
│   │   ├── Cargo.toml      # Depends on dem_io
│   │   └── src/lib.rs      # Normals, shadows, tiling, Morton encoding
│   ├── render_cpu/
│   │   ├── Cargo.toml      # Depends on terrain, rayon
│   │   └── src/lib.rs      # Raymarcher, SIMD packet tracing, shading
│   ├── render_gpu/
│   │   ├── Cargo.toml      # Depends on terrain, wgpu
│   │   └── src/lib.rs      # wgpu compute pipeline, WGSL shaders
│   └── profiling/
│       ├── Cargo.toml      # Minimal — maybe just libc for rdtsc
│       └── src/lib.rs      # Cycle counters, CSV emitter, perf-stat wrappers
├── benches/
│   ├── normals.rs          # Criterion benchmarks for Phase 2
│   ├── shadows.rs          # Criterion benchmarks for Phase 3
│   └── raymarcher.rs       # Criterion benchmarks for Phase 4
├── scripts/
│   ├── perf_stat.sh        # Wrapper to run with perf counters
│   └── instruments.sh      # Wrapper for macOS Instruments
├── data/                   # .hgt files (gitignored, ~32-64 MB each)
└── docs/
    └── planning/
        └── global_plan.md  # Global phased plan
```

---

## Why This Specific Decomposition

### `dem_io` — Data Layer

- **Zero external dependencies** for the core parser. An `.hgt` file is literally `width × height × sizeof(i16)` bytes, big-endian. You read the file, byte-swap on little-endian (which both x86 and ARM are), and you have a flat array.
- This crate owns the **memory layout decision** — it's where you implement tiled storage, Morton encoding, and aligned allocation. The rest of the crates see an abstract `Heightmap` type, but the layout underneath is what determines cache behavior.
- **Why separate?** You want to benchmark I/O + layout independently. "How long does it take to parse + tile 64 MB?" is a meaningful question on its own. Also, both CPU and GPU renderers consume the same heightmap — it's shared data.

### `terrain` — Compute Kernels

- **Normals + shadows** — the two main compute-heavy operations that happen *before* rendering. These are where SIMD + cache optimization matter most because they touch every pixel.
- Depends on `dem_io` to access the heightmap. Outputs normal buffers (SoA `Vec<f32>`) and shadow masks.
- **Why separate from rendering?** Both CPU and GPU renderers consume the precomputed normals/shadows. If you bake them into `render_cpu`, you'd duplicate for GPU. Separation also lets you benchmark the compute kernels alone.

### `render_cpu` — CPU Raymarcher

- Depends on `terrain` (for normals/shadows) and `rayon` (thread pool).
- This is where packet raytracing, screen-space tiling, and the main SIMD inner loop live.
- **Why its own crate?** You'll iterate on this heavily and want fast recompilation. Changes here shouldn't trigger recompilation of `dem_io` or `terrain`.

### `render_gpu` — GPU Path

- Depends on `terrain` + `wgpu`. The WGSL shader source lives here (as a string or `.wgsl` file).
- **Optional via feature flag** — in the workspace root `Cargo.toml`, you can make this a default member that's easy to exclude: `cargo build --workspace --exclude render_gpu` when you're focused on CPU work.

### `profiling` — Measurement Infrastructure

- Cycle-counting macros (`rdtsc` on x86, `cntvct_el0` on ARM), CSV output, maybe helpers that invoke `perf stat` and parse the output.
- **Every other crate depends on this** (or it's a dev-dependency for benchmarks). This ensures measurement is pervasive, not bolted on.

---

## Dependency Graph

Types like `Heightmap`, `NormalMap`, `ShadowMask`, `Camera` are consumed by multiple crates. Two options:

- **Option A**: Define them in the crate that produces them (`Heightmap` in `dem_io`, `NormalMap` in `terrain`). Downstream crates depend on upstream. This is a clean DAG.
- **Option B**: A shared `types` crate that everyone depends on.

Option A is simpler and avoids a "god types" crate. The dependency DAG is:

```
profiling (leaf — no deps)
    ↑
dem_io (depends on profiling)
    ↑
terrain (depends on dem_io, profiling)
    ↑         ↑
render_cpu   render_gpu  (each depends on terrain, profiling)
    ↑         ↑
  main.rs (depends on everything)
```

---

## Inlining Across Crate Boundaries

By default, Rust doesn't inline across crate boundaries unless you use `#[inline]` or enable LTO. For hot paths (SIMD kernels in `terrain` called from `render_cpu`), this matters. Options:

- Mark hot functions `#[inline]` — compiler *can* inline them across crates
- Enable **thin LTO** in release profile — `lto = "thin"` in `Cargo.toml` `[profile.release]`
- Enable **fat LTO** for final benchmarks — `lto = "fat"`, slower to compile but maximum optimization
- For profiling, use `#[inline(never)]` on the function you're measuring so it shows up as a distinct symbol in `perf report`

This is a real tension: you want inlining for performance but non-inlining for profiling visibility. The convention is: develop with `#[inline(never)]`, benchmark both ways, use LTO for final numbers.

---

## Build Profiles

```toml
[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 1    # Better optimization, slower compile
target-cpu = "native" # Use via RUSTFLAGS="-C target-cpu=native"

[profile.bench]
inherits = "release"
debug = true          # Symbols for perf report
```

- **`codegen-units = 1`** is important — with multiple codegen units, the compiler splits each crate into chunks and optimizes them independently, which can miss cross-function optimizations. For benchmarking, you want `1`.
- **`target-cpu = native`** (passed via `RUSTFLAGS`) enables the compiler to use AVX2/AVX-512/NEON based on your actual hardware. Without it, it targets a conservative baseline.
- **`debug = true` in bench profile** — keeps debug symbols so `perf report` / Instruments can show function names and source lines. Does not affect optimization.
