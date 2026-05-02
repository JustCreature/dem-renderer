# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

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

## Project Purpose

A learning-first, cache-optimized terrain + sunlight renderer in Rust using real USGS DEM data (~4000×4000, ~32–64 MB). The explicit goal is deep hardware understanding — memory hierarchy, SIMD utilization, TLB behavior, store buffers, ROB limits, branch predictor internals — not just producing a working renderer. Every design decision must be justified at the microarchitectural level, and every optimization must be validated with measured numbers.

---

## Architecture

### Workspace Structure

```
dem_renderer/
├── Cargo.toml
├── build.rs
├── src/
│   ├── main.rs
│   ├── system_info.rs
│   ├── utils.rs
│   └── viewer/
│       ├── mod.rs          # winit app loop, WASD+mouse, sun animation, tile sliding
│       ├── geo.rs          # CRS forward/inverse projections
│       ├── hud_renderer.rs # glyphon HUD, sun indicator, settings panel
│       ├── scene_init.rs   # GpuScene construction + tier wiring
│       ├── tiers.rs        # StreamingTier, BEV 1m tier state
│       ├── shader_hud_bg.wgsl
│       └── shader_sun_hud.wgsl  # SDF season/time circles
├── crates/
│   ├── dem_io/
│   │   └── src/
│   │       ├── heightmap.rs  # Heightmap, parse_bil, fill_nodata, parse_hdr
│   │       ├── geotiff.rs    # GeoTIFF parsing, extract_window, CRS projections
│   │       ├── grid.rs       # assemble_grid, load_grid, crop (GLO-30 3×3 assembly)
│   │       └── lib.rs
│   ├── terrain/
│   │   └── src/
│   │       ├── lib.rs           # NormalMap (SoA), ShadowMask, compute_ao_true_hemi
│   │       ├── row_major.rs     # compute_normals_scalar/_neon/_neon_parallel
│   │       ├── row_major_avx2.rs
│   │       ├── shadow.rs        # DDA shadow sweep, NEON variants
│   │       └── shadow_avx2.rs
│   ├── render_gpu/
│   │   └── src/
│   │       ├── context.rs       # GpuContext (device, queue, instance, adapter)
│   │       ├── camera.rs        # CameraUniforms (256 bytes, std140)
│   │       ├── vector_utils.rs
│   │       ├── render_rexture.rs
│   │       ├── lib.rs
│   │       ├── shader_texture.wgsl  # raymarcher: 3-tier height blend, AO, fog, LOD
│   │       └── scene/
│   │           ├── mod.rs       # GpuScene, dispatch_frame, resize, update_heightmap
│   │           ├── bind_group.rs  # rebuild_bind_group (22-entry canonical BG)
│   │           └── tiers.rs     # upload_hm5m/hm1m, set_*_inactive
│   └── profiling/
│       └── src/lib.rs           # cntvct_el0 / rdtsc, timed(), CSV emit
├── tiles/                        # Copernicus GLO-30 COG + BEV 1m/5m tiles (gitignored)
│   ├── Copernicus_DSM_COG_10_N*/
│   └── big_size/                # 1m_innsbruck_area, 1m_salzburg_south_area, 5m_whole_austria
├── n47_e011_1arc_v3_bil/         # SRTM BIL source data (gitignored)
└── docs/
    ├── planning/
    ├── lessons/
    ├── sessions/
    ├── gems/
    └── benchmark_results/
```

### Dependency DAG

```
profiling (leaf)
    ↑
dem_io
    ↑
terrain
    ↑
render_gpu
    ↑
  main.rs
```

Types are defined in the crate that produces them: `Heightmap` in `dem_io`, `NormalMap`/`ShadowMask` in `terrain`.

### Crate Responsibilities

- **`dem_io`**: Parse SRTM `.hgt` and GeoTIFF files. GLO-30 3×3 grid assembly. Selective COG window reads (`extract_window`). CRS: EPSG:4326, 31287, 3035.
- **`terrain`**: Surface normals (SoA finite differences, NEON + AVX2). Sun shadow sweep (DDA horizon-angle, NEON + AVX2). AO (16-azimuth DDA, averaged).
- **`render_gpu`**: wgpu texture-based raymarcher. `GpuScene` holds all GPU resources persistently. Three resolution tiers (30m / 5m / 1m) blended in WGSL. Swap-chain viewer lives in `src/viewer/`.
- **`profiling`**: `cntvct_el0` (AArch64) / `rdtsc` (x86) cycle counters, CSV timing emitter.

---

## Interaction Mode

- **Guide.** The user is building this to learn. Explain *why* something works at the hardware level, point to the right direction, suggest experiments — but do not write code or execute commands unless explicitly asked.
- **Assume strong technical curiosity.** The user wants full-depth explanations: cache-line math, TLB reach calculations, ROB/store-buffer reasoning, branch predictor behavior. Don't simplify unless asked.
- **Encourage measurement over intuition.** When the user asks "which is faster?", the answer is almost always "profile it — here's how and what counters to look at."
- **Build layered mental models.** Start from the hardware constraint (cache size, SIMD width, pipeline depth), derive the software implication, then suggest the experiment to validate.
- **Go full hardware depth.** Reason about store buffers, ROB size, retirement rate, branch predictor internals (TAGE), TLB pressure, prefetcher training, port pressure — not just "use SIMD and cache lines."

---

## Key Measurement Results (M4 Max unless noted)

### Memory bandwidth baseline (256 MB working set)
- seq_read scalar: 5.7–6.7 GB/s | seq_read SIMD: 21.8–37 GB/s
- random_read scalar: 0.6 GB/s | random_read SIMD: 1.4 GB/s
- Sequential/random ratio: 11–16× — drives all tiling decisions

### Normal computation (cold cache, 3601×3601)
- Scalar (black_box): 8.1 GB/s | Auto-vectorized: 24.1 GB/s | NEON 4-wide: 28.8 GB/s
- NEON parallel (10 cores): 42–50 GB/s cold | ~117 GB/s warm
- Tiled NEON parallel: 34 GB/s (worse — output is row-major, writes dominate)
- Stencil row-major: 60–72 GB/s (auto-vec 8-wide) | tiled: ~11 GB/s (`continue` blocks vec)

### Shadow computation (cold cache)
- Scalar branchy: 8.1 GB/s | Scalar branchless: 10.6 GB/s | NEON 4-wide: 17.4 GB/s | NEON parallel: 58.6 GB/s
- Diagonal shadow 2.4× slower than cardinal (strided cache-line access)
- CPU NEON parallel: 1.5 ms | GPU shadow: 26 ms — CPU wins 17× (serial running-max dependency)

### CPU raymarcher (2000×900 image)
- Scalar single-thread: 0.80s | NEON single-thread: 0.80s (gather overhead cancels SIMD gain)
- Scalar parallel (10 cores): 0.08s — 10× speedup, near-ideal scaling
- Average steps per ray: 506 (≈10.5 km); effective read rate: ~22 GB/s (compute-bound, not BW-bound)

### GPU rendering (8000×2667 = 21.3 Mpix)
- GPU buffer: 130 ms | GPU texture: 170 ms | GPU combined (normals on GPU): 90 ms | CPU parallel: 1260 ms
- Multi-frame per-frame: GPU scene 98 ms | GPU combined 120 ms | GPU separate 133 ms | CPU 1730 ms
- GpuScene speedup over CPU: 17.7× — 85 MB readback was the hard floor (~88 ms)

### Swap-chain viewer (no readback, 1600×533)
- M4: **477 fps** (2.1 ms/frame) | Win GTX 1650: **260 fps** | Mac i7: **53 fps** | Asus Pentium: **4.9 fps**
- vs bench_fps with readback: M4 10.3× | Win 24.1× | Mac i7 11.3× | Asus 7.1×
- True compute at 8000×2667: **21 fps** (47 ms)
- Command overhead floor: ~2.1 ms (~470 fps) — fixed cost regardless of shader work

### Thread / accumulator scaling (3601×3601)
- Thread scaling writes: linear to 8T (85 GB/s), ceiling at 12T (101 GB/s)
- Thread scaling reads: linear to 10T (259 GB/s), ceiling at 12T (247 GB/s) — write 3× narrower than read
- NEON 1-acc: 18 GB/s | 4-acc: 71 GB/s | 8-acc: 120 GB/s (SLC-bound)
- TLB knees: 4 MB (L1 DTLB, 256×16 KB) and 16–64 MB (L2 TLB, ~48 MB)

### AoS vs SoA / Morton
- AoS vs SoA: 1.00× single-thread | 1.13× parallel (barely BW-limited on M4)
- SoA advantage scales with bandwidth starvation: Win 2.3× | Asus 2.7×
- Morton vs row-major tiled: 1.00× — OOO ROB hides L2 latency, never reaches DRAM
- Software prefetch: +14% max at D=64 — M4 ROB (~600) already issues speculative loads

### Multi-tile loading (10800×10800 assembled GLO-30 grid)
- load_grid (9 × DEFLATE COG from disk): 4.52 s | normals: 185 ms | shadows: 525 ms
- AO full grid (16-azimuth DDA): 7.81 s | AO cropped (20 km radius): 290 ms — **27× speedup**
- `extract_window` (5m BEV DGM, 5 km radius, cold): **18.6 ms** — ~64 tiles read out of ~128,000 (0.05% of file)

### Cross-system (Win Nitro i5+GTX1650 / Mac Intel i7 / Asus Pentium N3700)
- Auto-vec penalty universal: 6.5–10× on every machine (same root cause, different ISA)
- Write/read asymmetry: M4 0.40 | Mac i7 0.26 | Asus 0.33 | Win 0.16 (write-allocate RFO)
- TLB: x86 exhausts at 1 MB (256 × 4 KB); M4 exhausts at 4 MB (256 × 16 KB)
- GTX1650: compute ~20 ms, PCIe readback ~47 ms → fps ceiling is PCIe BW, not shader throughput

---

## Key Lessons Learned

### Vectorization
- A single `continue` in the inner loop cuts throughput 6× regardless of ISA, tile size, or thread count
- Compiler auto-vectorization is powerful but fragile — one control-flow escape gates everything
- Tiling helps input reads but hurts output writes when output layout doesn't match iteration order

### Memory layout
- `get()` abstraction overhead dominates in tight loops — must use direct tile pointer arithmetic to see tiling benefit
- Write path saturates at fewer threads than read path on every machine (RFO + store buffer)
- M4 16 KB pages give 4× TLB reach vs x86 — critical at large working sets (26 MB heightmap)
- Morton ordering needs DRAM pressure to matter; OOO ROB hides the L2 latency difference

### GPU vs CPU
- GPU wins rendering (17.7×) — raymarching is embarrassingly parallel, no inter-pixel dependencies
- CPU wins shadow — running-max is serial per row; GPU shader cores run at lower clock with no advantage
- PCIe readback is the fps ceiling on discrete GPU; unified memory (M4) eliminates this tax entirely
- Swap chain removes 96% of perceived frame time on GTX 1650

### wgpu specifics
- Bind groups store GPU addresses, not CPU-side Arc refs — all referenced resources must be kept alive in the owning struct
- `write_buffer` updates buffer contents in-place; bound bind group sees new data automatically on next dispatch
- Default buffer binding limit 128 MB; fix: `required_limits: adapter.limits()`
- Texture dimension limit 8192 px (hardware max, not wgpu default)
- wgpu does not expose VkSparseBinding or Metal sparse textures; software indirection is the only option
- Workgroup size (64–256 threads, 8×8 to 32×8): all within ±3% when readback dominates

### GeoTIFF / CRS
- Pixel-scale tag value distinguishes geographic CRS (<0.1 deg/px) from projected (≥1.0 m/px) at load time
- EPSG:3035 LAEA Europe: FE=4321000, FN=3210000, lat0=52°N, lon0=10°E, GRS 1980
- EPSG:31287 Austria Lambert: FE=FN=400000, lat0=47.5°N, lon0=13.333°E, Bessel 1841; 5m/px
- `tiff` crate default memory limit blocks tiles > 128 MB; fix: `Limits::unlimited()`
- BEV DGM 5m NoData sentinel = 0.0 (safe: min Austrian elevation >> 0)
- Tile geometry at mid-latitudes is asymmetric: E-W width shrinks with cos(lat)
- GLO-30 tiles: 3600×3600 pixel-is-area, pixel centres at ±0.5/3600° from integer degree boundary; adjacent tiles concatenate directly

### Multi-resolution tiers
- True Hemisphere AO = sun shadow DDA generalised: 16 azimuths, averaged — baked once, free at render time
- HBAO radial 600m sweep exposes smaller GPU caches (GTX 1650); SSAO fixed-offset samples stay cache-local
- C1 discontinuity (slope jumps at DEM grid lines) is a data floor; Gaussian smoothing fixes the symptom but destroys ridgelines
- At 47°N: SRTM tiles are 111 km N-S × 76 km E-W; fog 60 km always overshoots E/W edges → 3×3 tile grid required

---

## Open Items

- GPU shadow via parallel prefix scan (deferred multiple times)
- Occupancy analysis via Instruments/Metal GPU trace — requires full Xcode.app
- `fill_nodata` division-by-zero if all 4 directions hit boundary without finding valid data
- Supersampled ray optimization: march 1 reference ray, approximate 3 neighbours via gradient. Breaks at sharp peaks.

---

## Build Commands

```sh
cargo build --release
cargo bench -p terrain
RUSTFLAGS="-C target-cpu=native" cargo build --release  # Enable AVX2/NEON
```

**Build profiles** (workspace `Cargo.toml`):
```toml
[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 1

[profile.bench]
inherits = "release"
debug = true  # symbols for perf report / Instruments
```

Use `#[inline(never)]` during profiling so functions appear as distinct symbols. Switch to `#[inline]` + LTO for final benchmark numbers.

---

## Key Design Decisions

| Decision | Rationale |
|---|---|
| SoA over AoS for normals | Load 8 consecutive nx values in one AVX2 instruction |
| Branchless inner loops | SIMD masks / `cmov` in shadow sweep, ray termination |
| `#[inline(never)]` on profiled functions | Distinct symbols in `perf report` / Instruments |
| `codegen-units = 1` | No cross-function optimization loss |
| Thin LTO in release | Cross-crate inlining for hot paths |
| CPU shadow, GPU render | Running-max is serial → CPU wins; raymarching embarrassingly parallel → GPU wins |
| Swap-chain viewer | Eliminates 85 MB readback floor; PCIe was the fps bottleneck, not shader compute |
| Multi-resolution tiers | 30m GLO-30 base + 5m BEV detail + 1m BEV fine; blended in shader |
| Single canonical bind group | `rebuild_bind_group()` — one 22-entry BG replaces 4 duplicate ~90-line blocks |

---

## Coding Conventions

- Rust stable; nightly only for `std::simd` / `core::arch` not yet stabilized.
- `unsafe` only for SIMD intrinsics — document the safety invariant inline.
- Prefer `core::arch` over `std::simd` when stable intrinsics cover the operation.
- Name SIMD dispatch functions explicitly: `compute_normals_neon()`, `compute_normals_avx2()`, `compute_normals()` (dispatcher).

---

## Profiling

**Target hardware**: Apple Silicon M4 Max (NEON, 128 KB L1D) and x86-64 (AVX2, 32–48 KB L1D).

Before claiming an optimization works, measure:
- Wall-clock (Criterion or manual)
- `perf stat`: cycles, instructions, IPC
- L1/L2/L3 miss rates, dTLB miss rate, branch misprediction rate
- Apple Silicon: Instruments CPU Counters template

Key counters: `cache-misses`, `L1-dcache-load-misses`, `dTLB-load-misses`, `instructions`, `cycles`, `branches`, `branch-misses`, `resource_stalls.sb`, `fp_ret_sse_avx_ops.all`.
