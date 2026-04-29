⛔ NEVER write code, edit files, or run commands without explicitly announcing Code Exception Mode first. This is a learning project — guide, don't implement.

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

**Current phase: Phase 9** (Phases 0, 1, 2, 3, 4, 5, 6, 7, 8 complete)

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

Phase 6 artifacts:
- `src/benchmarks/phase6.rs` — all 9 experiments
- `docs/sessions/phase-6/main-session.md` — session log (2 sessions)
- `docs/lessons/phase-6/long-report.md` — comprehensive Phase 6 student textbook (includes cross-system synthesis)
- `docs/lessons/phase-6/short-report.md` — Phase 6 reference card (includes cross-system synthesis)
- `docs/benchmark_results/report_1/` — all CSV data + interactive HTML + MD report for all 4 machines
- `skills/learning-guide.skill` — repackaged with stricter code-exception enforcement

Phase 6 key numbers (M4 Max, 3601×3601, cold cache, 2026-04-06):
- Stencil row-major: 60–72 GB/s (auto-vec 8-wide NEON) | tiled all sizes: ~11 GB/s (`continue` blocks vec)
- Thread scaling writes: linear to 8T (85 GB/s), ceiling at 12T (101 GB/s)
- Thread scaling reads: linear to 10T (259 GB/s), ceiling at 12T (247 GB/s) — write 3× narrower than read
- AoS vs SoA: 1.00× single-thread | 1.13× parallel (barely BW-limited on M4)
- Morton vs row-major tiled: 1.00× — OOO ROB hides L2 latency, never reaches DRAM
- Software prefetch: +14% max at D=64 — M4 ROB (~600) already issues speculative loads
- NEON 1-acc: 18 GB/s | 4-acc: 71 GB/s | 8-acc: 120 GB/s (SLC-bound)
- TLB knees: 4 MB (L1 DTLB, 256×16KB) and 16–64 MB (L2 TLB, ~48 MB)

Cross-system key numbers (Win Nitro i5+GTX1650 / Mac Intel i7 / Asus Pentium N3700, 2026-04-09):
- Auto-vec penalty universal: 6.5–10× on every machine (same root cause, different ISA)
- Write/read asymmetry: M4 0.40 | Mac i7 0.26 | Asus 0.33 | Win 0.16 (write-allocate RFO)
- TLB: x86 exhausts at 1 MB (256 entries × 4 KB); M4 exhausts at 4 MB (256 entries × 16 KB)
- FPS benchmark (1600×533): M4 46.4 fps | Win GTX1650 15.2 fps | Mac i7 11.8 fps | Asus 4.5 fps
- GTX1650: compute ~20 ms, PCIe readback ~47 ms → 15 fps measures PCIe BW not shader throughput
- SoA advantage: M4 1.13× parallel | Win 2.3× | Asus 2.7× — scales with bandwidth starvation

Phase 6 lessons:
- Vectorisation gates everything — a single `continue` cut throughput 6× regardless of ISA, tile size, or thread count
- Write path saturates at fewer threads than read path on every machine (RFO + store buffer)
- SoA advantage is invisible when compute-bound; grows to 2–3× when bandwidth-starved
- M4 16 KB pages give 4× TLB reach vs x86 — critical at large working sets (26 MB heightmap)
- PCIe readback is the fps ceiling on discrete GPU; unified memory (M4) eliminates this tax entirely
- Serial reduction chains need multiple accumulators; Morton ordering needs DRAM pressure to matter

Phase 7 artifacts:
- `src/viewer/mod.rs` — interactive swap-chain viewer: winit 0.30 `ApplicationHandler`, WASD + mouse look, vsync toggle (`--vsync`), immersive mode (Q), left-click drag look, FPS counter, window resize (`render_width` alignment), HUD toggle (E), speed boost (Cmd/Alt), sun animation (+/-/[/] keys), `sim_day`/`sim_hour`/`day_accum` fields
- `src/viewer/hud_renderer.rs` — `HudBackground`, `SunIndicator`, `HudRenderer`; 10 glyphon text buffers; `day_to_date()` date formatter; drop shadows (double-render at +1,+1); settings HUD panel (AO label top-right)
- `src/viewer/shader_sun_hud.wgsl` — SDF season/time circles; `season_col()`, `panel_rect_sdf()`, `draw_circle()`; `discard` guard; fully commented
- `src/main.rs` — `--view` / `--vsync` CLI flags
- `crates/render_gpu/src/scene.rs` — `dispatch_frame(&mut encoder)`, `resize()`, `render_bgl` stored; R16Float heightmap texture + `half` crate upload; 7-level max-filter mip generation; AO R8Unorm texture (bindings 8+9); `GpuScene::new()` takes `normal_map` + `ao_data_mask`
- `crates/render_gpu/src/context.rs` — `pub instance` and `pub adapter` fields
- `crates/render_gpu/src/shader_texture.wgsl` — BGRA output; bilinear height (`textureSampleLevel`); bilinear normals/shadows; smooth color bands; fog (15–60km); sphere tracing (adaptive step, sky early exit, `t_prev` bracket); AO modes 1–5; step LOD (`0.7 + t/8000`); mip LOD (`log2(1+t/15000)`)
- `crates/render_gpu/src/shader_buffer.wgsl` — parity update: bilinear `sample_hm()`, smooth colors, fog
- `crates/render_cpu/src/lib.rs` — `shade()`: bilinear normals/shadows, smooth elevation colors
- `crates/render_gpu/Cargo.toml` — `half = "2"` for R16Float upload
- `crates/terrain/src/lib.rs` — `compute_ao_true_hemi(hm, 16, 5°, 200m)`: 16-azimuth DDA sweep → averaged lit fraction
- `crates/terrain/src/shadow.rs` + `shadow_avx2.rs` — `.round()` → `.floor()` fix (22 sites); `penumbra_meters` soft shadow parameter added to all variants
- `src/benchmarks/phase6.rs` — GPU no-readback variant added
- `docs/benchmark_results/report_1/fps_no_readback.csv` + updated `report_1.md` / `report_1.html`
- `docs/planning/viewer-improvements-plan.md` — AO + LOD roadmap; tile streaming item struck through (moved to phase-8)
- `docs/planning/viewer-phase-8.md` — phase 8 roadmap: shadow toggle, fog toggle, VAT presets, LOD distance presets, tile streaming
- `docs/lessons/phase-7/long-report.md` + `short-report.md`
- `docs/sessions/phase-7/main-session.md` — session log

Phase 7 key numbers (M4 Max, 1600×533, 2026-04-11 to 2026-04-18):
- Viewer swap-chain (no readback): **477 fps** (2.1ms/frame) vs bench_fps 10fps — 46× proves readback was the bottleneck
- Command overhead floor: ~2.1ms (~470fps) — fixed cost regardless of shader work
- True compute at 8000×2667: **21 fps** (47ms) — Phase 5 "10ms compute" was wrong (readback overlapped)
- vsync on: 100fps (display-capped); vsync off: 477fps at 1600×533
- AO modes on M4: no perceptible fps difference across all 6 modes (13 MB heightmap fits in SLC)
- AO on GTX 1650 (~50fps baseline): SSAO×8 −2–3fps | SSAO×16 −8fps | HBAO×4 −20fps | HBAO×8 −25fps | True Hemi 0fps
- Step LOD + mipmap LOD: no fps change on M4 (cache-bound); expected gain on GTX 1650 (unmeasured)

No-readback cross-system (1600×533, bench_fps, 2026-04-11):
- M4: **477 fps** | readback 10.3× | GPU vs CPU 32×
- Win GTX 1650: **260 fps** | readback 24.1× | GPU vs CPU 298×
- Mac i7: **53 fps** | readback 11.3× | GPU vs CPU 36×
- Asus Pentium: **4.9 fps** | readback 7.1× | GPU vs CPU 32×

Phase 7 lessons:
- Swap chain removes 96% of perceived frame time on GTX 1650 — PCIe readback was the bottleneck, not shader compute
- M4 SLC (~48 MB+) absorbs the entire 13 MB heightmap — LOD and AO cache effects are invisible until tested on GTX 1650
- HBAO's radial 600m sweep (96–192 samples spread across large UV area) exposes GTX 1650's smaller GDDR6 cache; SSAO's fixed-offset samples stay cache-local
- True Hemisphere AO = sun shadow DDA generalised: same function, 16 azimuths, averaged — baked once at startup, free at render time
- C1 discontinuity (slope jumps at 20m DEM grid lines) is a data floor; Gaussian smoothing fixes the symptom but destroys real ridgelines

Known open items carried into Phase 8:
- GPU shadow via parallel prefix scan — deferred from Phase 5
- `render_gif::render_gif` commented out in main.rs — deferred from Phase 5
- Occupancy analysis via Instruments/Metal GPU trace — requires full Xcode.app (deferred from Phase 5)

Phase 8 artifacts:
- `docs/planning/viewer-phase-8.md` — Part 0 data source plan + viewer roadmap (Item 5 struck through)
- `docs/planning/multi-tile-multiple-resolution-load.md` — 5-step multi-tile multi-resolution plan
- `docs/sessions/phase-8/main-session.md` — session log (2026-04-20, 2026-04-25 ×3)
- `crates/dem_io/src/geotiff.rs` — `geotiff_pixel_scale()`, `parse_geotiff_epsg_3035()`, `laea_epsg3035_inverse()`; `Limits::unlimited()` in EPSG:3035 parser
- `crates/dem_io/src/heightmap.rs` — `Heightmap` extended: `crs_origin_x`, `crs_origin_y`, `crs_epsg`
- `crates/render_gpu/src/context.rs` — `required_limits: adapter.limits()` to unlock full hardware buffer sizes
- `src/viewer/mod.rs` — auto-dispatch by pixel scale; `lcc_epsg31287()`, `laea_epsg3035()` forward projections; `latlon_to_tile_metres()` dispatching on `crs_epsg`; named camera position; shadow/fog/vat/lod toggles (`.`, `,`, `;`, `'` keys)
- `src/viewer/hud_renderer.rs` — 5-line settings panel (AO + shadows + fog + quality + LOD); background rect sized to match
- `crates/render_gpu/src/camera.rs` — `CameraUniforms` extended with `shadows_enabled`, `fog_enabled`, `vat_mode`, `lod_mode`
- `crates/render_gpu/src/shader_texture.wgsl` — shadow/fog/vat/lod shader logic from uniform fields
- DEM tiles: `hintertux_5m.tif`, `hintertux_18km_5m.tif`, `hintertux_8km_1m.tif` (EPSG:3035, 8001×8001)

Phase 8 key facts:
- EPSG:3035 LAEA Europe: FE=4321000, FN=3210000, lat0=52°N, lon0=10°E, GRS 1980; scale = metres directly
- EPSG:31287 Austria Lambert: FE=FN=400000, lat0=47.5°N, lon0=13.333°E, Bessel 1841; 5m/pixel
- wgpu default buffer binding limit 128 MB; fix: `adapter.limits()`; texture dimension limit 8192 px
- `tiff` crate default memory limit: fix `Limits::unlimited()` for tiles > 128 MB
- BEV DGM 5m NoData sentinel = 0.0 (safe: min Austrian elevation >> 0)
- Hintertux centre: WGS84 47.076211°N 11.687592°E → EPSG:31287 (273605, 356962) → EPSG:3035 (4449262, 2663978)
- `hintertux_8km_1m.tif` confirmed rendering correctly (8001×8001, EPSG:3035, 1m/px)
- Out-of-bounds ray fix (remove bounds break, use `in_bounds` guard + 5× step): correct visually
  but fps-prohibitive — reverted. Hard break is the right trade-off.
- At 47°N: SRTM tiles are 111 km N-S × 76 km E-W; fog 60 km always overshoots E/W edges →
  3×3 tile grid required; 10801 px assembled > wgpu 8192 limit → outer 8 tiles at half-res (5401 px)
- wgpu does not expose VkSparseBinding or Metal sparse textures; software indirection is the only
  option within the wgpu abstraction layer

Phase 8 lessons:
- GeoTIFF CRS diversity (EPSG:4326, 31287, 3035) requires per-CRS forward+inverse projections;
  pixel-scale tag value distinguishes geographic (<0.1) from projected (≥1.0) at load time
- wgpu resource limits have safe defaults far below hardware maximums; always request
  `adapter.limits()` for production use with large data
- Tile geometry at mid-latitudes is asymmetric: E-W width shrinks with cos(lat), making E/W
  neighbours mandatory and N/S neighbours conditional on camera position within the tile
- Half-resolution preprocessing for outer tiles is justified: detail beyond ~38 km is invisible
  at 30m resolution and within the fog blend zone anyway
- Software page tables are the correct abstraction when hardware sparse textures are unavailable;
  the indirection cost is negligible compared to texture sample latency

Known open items carried into Phase 9:
- Multi-tile multi-resolution streaming — full plan in `docs/planning/multi-tile-multiple-resolution-load.md`
  - Step 1: 30m 3×3 sliding window (download 8 surrounding Copernicus tiles first:
    N46E010, N46E011, N46E012, N47E010, N47E012, N48E010, N48E011, N48E012)
  - Step 2: windowed GeoTIFF extraction (in-process `-projwin`)
  - Step 3: per-tier background loader threads with coarse fallback
  - Step 4: multi-source-tile stitching
  - Step 5: multi-tier shader with lerp blend zones
- GPU shadow via parallel prefix scan — deferred from Phase 5
- `render_gif::render_gif` commented out in main.rs — deferred from Phase 5

Phase 9 artifacts (in progress):
- `docs/sessions/phase-9/main-session.md` — session log
- `crates/dem_io/src/grid.rs` — `assemble_grid`, `load_grid<F>`, `tile_path`, `crop`; 3×3 Copernicus GLO-30 grid assembly + in-memory crop
- `crates/dem_io/src/lib.rs` — `crop` re-exported alongside `assemble_grid`, `load_grid`
- `crates/render_gpu/src/scene.rs` — `_ao_texture` stored; normal buffers gain `COPY_DST`; `write_hm_mips` free function; `update_heightmap(&mut self, hm, normals, ao)` implemented; `_hm1m_*` fields + bindings 16–21 + `upload_hm1m()` + `set_hm1m_inactive()` for 1m fine tier
- `src/viewer/mod.rs` — `parse_copernicus_lat_lon()`; `tile_meters_to_latlon_epsg_4326()`; `TileBundle`; background tile loader thread; per-frame crossing detection + cam_pos re-projection; shadow worker respawn on tile slide; `last_shadow_az/el` throttle (recompute only on ≥0.1° sun movement); dt fix (true inter-frame time); `compute_ao_cropped(hm, cam_lat, cam_lon)` free function; `AO_RADIUS_M = 20_000.0`; tile loader channel upgraded to `(i32, i32, f64, f64)`; `prepare_scene` takes `cam_lat, cam_lon`; 1m fine tier: `find_1m_tiles` (multi-tile overlap scan), `stitch_1m_windows`, worker with dynamic tile discovery + stitching, poll block, `--1m-tiles-dir` CLI arg
- `src/viewer/tiers.rs` — `BEV_1M_RADIUS_M = 3500.0`, `BEV_1M_DRIFT_THRESHOLD_M = 1000.0`; `fine: Option<StreamingTier>` on `BevBaseState`
- `src/viewer/geo.rs` — `laea_epsg3035_inverse` (spherical LAEA inverse); `lcc_epsg31287_inverse` (iterative LCC inverse, Bessel 1841, <10 iterations)
- `crates/render_gpu/src/camera.rs` — `CameraUniforms` extended with 8 hm1m fields (256 bytes total); initialized to 0/inactive in `new()`
- `crates/render_gpu/src/shader_texture.wgsl` — bindings 16–21 (hm1m tex/sampler/nx/ny/nz/shadow); `fine_tier_edge_dist` helper; three-tier height blend in raymarcher; three-tier normals/shadow blend in shading
- `src/main.rs` — `--1m-tiles-dir <path>` CLI arg; `viewer::run` signature updated
- `docs/planning/multi-tile-multiple-resolution-load.md` — AO radius optimisation note added to Step 3; texture dimension fallback added to Open Items
- `docs/planning/tmp/crop_extract.md` — Step 2 design doc: crop + extract_window; tiff crate API; window/tile math; algorithm
- `download_copernicus_tiles_30m.sh` — updated to 5×5 grid (lat 45–49, lon 9–13); skip-if-present check; printf zero-padding
- `crates/dem_io/src/geotiff.rs` — `extract_window(path, centre_crs, radius_m, ifd_level, crs_epsg)`; `laea_epsg31287_inverse` extracted; `extract_window` exported from `dem_io::lib`; edge-tile stride bug fixed (`actual_tw = tile_col1.min(cols) - tile_col0`)

Phase 9 key numbers so far (Intel Mac, 10800×10800 assembled grid, 2026-04-25 to 2026-04-28):
- load_grid (9 × DEFLATE COG from disk): 4.52s
- normals (parallel): 185ms
- shadows (parallel): 525ms
- AO full grid (16-azimuth DDA, 10800×10800): 7.81s
- AO cropped (20km radius, ~1334×1334 px): 290ms — **27× speedup**; pixel reduction 116M → ~1.78M
- Tiles: 3600×3600 pixel-is-area, pixel centres at ±0.5/3600° from integer degree boundary
- Adjacent tiles abut perfectly — 1/3600° spacing across boundary, simple concatenation
- Rendering verified correct: no seam artifacts, corner cases work, shader UV already dynamic
- Sliding window: background tile loader thread; crossing detected via floor(lon/lat) change;
  seamless cam_pos re-projection on slide; old shadow worker exits when sender dropped
- Shadow recompute bug: was uploading 466 MB every ~0.5s; fixed with 0.1° movement threshold
- AO staleness after tile slide: fixed via drift-based recompute — `ao_tx/ao_rx` worker thread,
  `AO_DRIFT_THRESHOLD_M` threshold, `update_ao()` on scene
- `extract_window` (5m BEV DGM, 5km radius, cold): **18.6ms**; 1707×1454 px output, elev 1398–3336m ✓
- ~64 internal 256×256 tiles read out of ~128,000 total; selective read = ~0.05% of file
- 1m tier: two 50km×50km tiles (Innsbruck E4400000, Salzburg E4450000); boundary at easting 4450000
- Edge-tile bug: 50001 mod 256 = 81 → last-column tiles are 81px wide; old stride=256 caused OOB panic
- 1m window: ±3500m = 7000×7000px; stitch combines both tiles when window straddles boundary
- `BEV_1M_RADIUS_M = 3500.0`, `BEV_1M_DRIFT_THRESHOLD_M = 1000.0`; GPU bindings 16–21
- Coordinate chain: EPSG:31287 → lcc_epsg31287_inverse → WGS84 → laea_epsg3035 → EPSG:3035 (worker);
  EPSG:3035 origin → laea_epsg3035_inverse → WGS84 → lcc_epsg31287 → tile-local offset (event loop)

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
