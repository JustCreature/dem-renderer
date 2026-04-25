# Plan: Cache-Optimized DEM Terrain + Sunlight Renderer

**TL;DR** — Build a heightmap renderer in Rust that loads real USGS DEM data (~4000×4000, ~32–64 MB), computes normals + sun shadows, and renders a shaded terrain image via two paths: a cache/SIMD/concurrency-optimized CPU raymarcher, and a wgpu GPU compute pipeline. Every phase is designed to expose a specific hardware concept — memory layout, cache-line utilization, SIMD port pressure, TLB reach, store buffers, ROB limits, branch prediction — and is profiled with `perf` (Linux/x86), Instruments (macOS/ARM), and manual cycle counting. The project is structured so each phase produces a working visual output, creating a feedback loop between optimization and visible results.

---

## Phase 0 — Foundations & Tooling Setup

1. **Cargo workspace structure**: one binary crate, multiple library crates (`dem_io`, `terrain`, `render_cpu`, `render_gpu`, `profiling`). Separate concerns so you can benchmark each in isolation.
2. **Profiling harness from day one**: integrate `perf stat` / `Instruments` invocation scripts, plus Rust-side `std::arch::x86_64::_rdtsc()` / `std::arch::aarch64::__cntvct_el0` wrappers for manual cycle measurement. Build a tiny `#[inline(never)]` timing macro that emits CSV.
3. **Baseline numbers**: write a trivial loop that touches 64 MB of `f32` sequentially and randomly. Measure bandwidth (should see ~40 GB/s sequential, ~1–2 GB/s random on modern HW). This calibrates your mental model of memory hierarchy cost.

**Hardware concepts**: memory bandwidth ceiling, `perf stat` counters (`cache-misses`, `L1-dcache-load-misses`, `dTLB-load-misses`, `instructions`, `cycles`, `branches`, `branch-misses`).

---

## Phase 1 — DEM Data Ingestion & Memory Layout

1. **Parse real data**: USGS SRTM `.hgt` / BIL files (`i16`, 1201×1201 or 3601×3601 per tile). Stitch 2–4 tiles to reach ~4000×4000. No external GeoTIFF library — parse the binary format yourself: read the `.hdr` for dimensions and byte order, then interpret the `.bil` as a flat array of `i16` values.

### Geo 101 — coordinate system, resolution, and data format

- **Arc-second**: 1 degree = 60 arc-minutes = 3600 arc-seconds. SRTM-1 samples every 1 arc-second. At the equator, 1 arc-second latitude ≈ 30.9 m. Longitude spacing shrinks by `cos(lat)` — at 47°N it is ~21 m east-west vs ~31 m north-south. **Pixels are not square on the ground** — critical for correct normal computation (the x and y finite differences must use the real-world cell sizes, not just pixel counts).
- **Tile naming**: tiles are named by their south-west corner. `N47E011` covers 47–48°N, 11–12°E. The first row in the file is the **north** edge (48°N), the last row is the south edge (47°N). Y increases southward in the array but northward in geography — a common source of upside-down renders.
- **3601 vs 3600**: a 1°×1° tile at 1 arc-second resolution has 3600 intervals → 3601 sample points (fencepost). The last row/column duplicates the first row/column of the adjacent tile. Drop it when stitching.
- **NODATA = -32767**: cells over ocean or where the radar had no return are marked -32767. Must be detected and handled before compute — passing -32767 as an elevation into normal or shadow calculations produces nonsense.
- **BIL format**: a `.bil` file is a raw flat binary array (row-major, north-to-south, west-to-east within each row). The `.hdr` sidecar declares `NROWS`, `NCOLS`, `NBITS`, `BYTEORDER` (I = little-endian, M = big-endian), `NODATA`. The `.blw` world file gives the pixel size in degrees and the origin. The classic `.hgt` format is identical but big-endian with no header (dimensions inferred from file size).
- **Cell size in meters** (needed for Phase 2 normals):
  - `dy_meters ≈ YDIM_degrees × 111,320` (latitude: ~constant)
  - `dx_meters ≈ XDIM_degrees × 111,320 × cos(lat_radians)` (longitude: shrinks with latitude)
  - For Hintertux (47°N): dy ≈ 30.9 m, dx ≈ 21.1 m

2. **Memory layout decision**: Store as a flat `Vec<i16>` in row-major order. Measure sizeof — 4000×4000×2 = ~32 MB. This fits in L3 but blows L2. Convert to `Vec<f32>` (64 MB) only when needed for compute — keep the cold copy small.
3. **Tile the data**: Divide into NxN tiles (experiment: 64×64, 128×128, 256×256). Store tiles in Z-order (Morton curve) or tile-linear order so that spatial locality maps to memory locality. The goal: when processing a region, all data fits in L2 or at worst L3.
4. **Alignment**: Ensure tile starts are 64-byte aligned (cache-line) and ideally 4096-byte aligned (page boundary) for prefetcher friendliness. Use `std::alloc::alloc` with `Layout::from_size_align`.

### Hardware concepts

- **Cache line utilization** — a 64×64 tile of `i16` = 8 KB = 128 cache lines. Fits in L1D (32KB x86, 128KB Apple Silicon).
- **TLB reach** — 64 entries × 4 KB pages = 256 KB L1 DTLB reach. A 64×64 `f32` tile = 16 KB = 4 pages = 4 TLB entries. Good. A 256×256 tile = 256 KB = 64 pages = entire L1 DTLB. Danger zone.
- **Huge pages** — if you use 2 MB pages, TLB reach jumps to 128 MB. Worth experimenting with `madvise(MADV_HUGEPAGE)`.
- **Prefetcher training** — row-major sequential access trains the hardware L2 streamer. Random tile access defeats it. Measure with `perf stat -e L2-prefetch-misses`.

---

## Phase 2 — Normal Computation (SIMD + Cache)

1. **Compute surface normals** from the heightmap using finite differences: for each pixel, `nx = h[x-1][y] - h[x+1][y]`, `ny = h[x][y-1] - h[x][y+1]`, `nz = 2*cellsize`. Normalize.
2. **Output layout**: `Vec<[f32; 4]>` (nx, ny, nz, padding) aligned to 16 bytes — one normal per pixel. 4000×4000×16 = 256 MB. Alternatively SoA: three separate `Vec<f32>` for nx, ny, nz (48 MB each, 192 MB total). **Design decision**: SoA is better for SIMD because you load 8 (AVX2) or 4 (NEON) consecutive nx values in one instruction.
3. **SIMD implementation**: Use `std::simd` (nightly) or `core::arch` intrinsics. Process 8 (AVX2) / 4 (NEON) normals per iteration. The finite-difference reads are offset by ±1 in x and ±row_stride in y — measure whether unaligned SIMD loads cause penalty (on modern HW: no penalty if within cache line, small penalty if crossing).
4. **Tiled processing**: Process one tile at a time. Load source tile + 1-pixel halo into L1/L2, write normals for that tile. This keeps working set small.
5. **Parallelism**: `rayon::par_iter` over tiles. Each thread works on independent tiles → no false sharing. Ensure tile output buffers are on different cache lines (64-byte spacing between tile starts).

### Hardware concepts

- **SIMD port pressure** — on Zen3, FP ADD runs on ports 0/1, FP MUL on 0/1, FP DIV only on port 0. The `rsqrt` approximation avoids the divider. On Apple M1, NEON has 4 FP pipes. Profile with `perf stat -e fp_ret_sse_avx_ops.all`.
- **Store buffer** — writing 192 MB of normals generates massive store traffic. Store buffer depth (56–72 entries) limits how far stores can get ahead of retirement. If computation is fast, you'll stall on stores. Measure with `resource_stalls.sb`.
- **False sharing** — if two threads write normals for adjacent tiles and those tiles' output buffers start on the same cache line, you get coherence traffic. Pad tile output to 64-byte boundaries.
- **ROB size** — the Reorder Buffer (256 Zen3, 512 Golden Cove, ~600+ M1) determines how many independent operations can be in-flight. The normal computation is highly parallel (each pixel independent) so the ROB should stay full. If it doesn't → you have a bottleneck.

---

## Phase 3 — Sun Shadow Computation

1. **Algorithm**: For each pixel, cast a ray toward the sun (defined by azimuth + elevation angles). March along the heightmap in the sun direction. If any sample is higher than the line from the pixel to the sun, the pixel is in shadow. This is a 2D DDA (Digital Differential Analyzer) across the heightmap.
2. **Optimization**: Process all pixels in the direction of the sun simultaneously — this is a "sweep" algorithm. For a sun coming from the west, sweep east-to-west, propagating the maximum horizon angle. O(N²) instead of O(N³). Access pattern becomes sequential in one direction.
3. **SIMD**: Process multiple rows (or columns) in parallel using SIMD. The horizon-angle sweep is a running-max operation — use `_mm256_max_ps` / `vmaxq_f32`.
4. **Output**: A `Vec<u8>` shadow mask (0 = shadow, 255 = lit) or `Vec<f32>` shadow factor for soft shadows. 4000×4000×1 = 16 MB.

### Hardware concepts

- **Branch prediction** — the "is this pixel shadowed?" test is a branch. If the sun angle creates a shadow boundary that's spatially coherent, the branch predictor (TAGE-like on all modern CPUs) will predict well except at the boundary. Measure `branch-misses`. Convert to branchless using conditional moves (`cmov`) or SIMD masks — compare branch vs branchless perf.
- **Prefetching** — the sweep direction is predictable. The hardware streamer should handle it. But the perpendicular dimension (multiple rows in flight) may benefit from software prefetch (`_mm_prefetch` / `__builtin_prefetch`). Experiment with prefetch distance = 2–4 cache lines ahead.
- **Instruction-level parallelism** — the sweep has a serial dependency (running max). This limits ILP. The ROB can't help if each iteration depends on the previous. Solution: interleave multiple independent sweeps (different rows) to fill the pipeline. This is a textbook example of latency-bound vs throughput-bound code.

---

## Phase 4 — CPU Renderer (Raymarching)

1. **Camera model**: define a pinhole camera with position, look-at, FOV. For each output pixel, cast a ray into the scene.
2. **Raymarching over heightmap**: step along the ray in XY, sampling the heightmap at each step. When the ray's Z drops below the height at that XY position, you've hit the terrain. Binary-search refinement for exact hit.
3. **Shading**: at the hit point, look up the precomputed normal and shadow mask. Apply Lambertian diffusion (`max(0, dot(normal, sun_dir))`) × shadow factor. Color by elevation gradient.
4. **Output**: write to a `Vec<u32>` RGBA framebuffer (4000×4000×4 = 64 MB for a 1:1 output, or smaller for a viewport).
5. **Display**: use `minifb` crate for a window, or write PNG via `image` crate.

**SIMD strategy**: process 8 (AVX2) / 4 (NEON) rays simultaneously. Each SIMD lane is an independent ray at a different pixel. All rays step together; lanes that have already hit the terrain are masked out. This is the "packet raytracing" approach.

### Hardware concepts

- **Cache access pattern** — rays from pixel (x, y) and pixel (x+1, y) sample nearby heightmap locations → spatial coherence → good cache behavior if rays are grouped in tiles. Rays from random pixels → cache thrashing. **Solution**: process rays in screen-space tiles (8×8 or 16×16) so nearby rays sample nearby heightmap data.
- **TLB pressure** — raymarching touches scattered heightmap memory. If the heightmap is 64 MB and you're jumping around, L1 DTLB (64 entries × 4 KB = 256 KB) is useless. **Fix**: tile the heightmap (Phase 1) + use huge pages. Measure `dTLB-load-misses`.
- **Divergence** — different rays hit at different steps. SIMD lanes become inactive early → wasted compute. This is the CPU analog of GPU warp divergence. Measure utilization (what fraction of SIMD lanes are active per step on average).
- **Retirement** — with complex control flow (ray termination, boundary checks), the ROB fills with speculative work. If branch misprediction rate is high, you flush the pipeline. Keep the inner loop branchless.

---

## Phase 5 — GPU Renderer (wgpu Compute)

1. **wgpu setup**: create a compute pipeline that reads the heightmap as a storage buffer, writes to an output texture.
2. **Compute shader (WGSL)**: each workgroup processes a tile of output pixels. Each invocation raymarches independently.
3. **Memory layout on GPU**: heightmap as a 2D texture (hardware bilinear filtering for free!) vs storage buffer (manual addressing). Compare performance: texture reads use the texture cache (optimized for 2D locality via Morton-order tiling internally on the GPU), buffer reads use L1/L2 cache.
4. **Workgroup sizing**: 8×8 = 64 threads (1 warp on NVIDIA, 1 wavefront on AMD, 1 SIMD-group on Apple GPU). This matches GPU execution width.
5. **Normal + shadow**: either precompute on CPU and upload, or compute on GPU. GPU shadow computation can use the sweep algorithm with shared memory barriers.

### Hardware concepts

- **GPU execution model** — 32 threads (warp/wavefront) execute in lockstep. Divergent rays → some threads idle → occupancy loss. Same problem as CPU SIMD but at larger scale.
- **Memory hierarchy difference** — GPU L1 is typically 16–128 KB per SM, shared memory is 48–100 KB per SM, L2 is 4–6 MB total. Different from CPU. Texture cache is specialized for 2D spatial locality.
- **Bandwidth** — GPU memory bandwidth is 200–900 GB/s vs CPU's 40–80 GB/s. For memory-bound workloads (heightmap sampling), GPU wins massively. But latency is higher (400–800 cycles GPU vs 4–100 cycles CPU for cache hits).
- **Occupancy** — GPU hides latency by switching between warps. Need enough warps in flight. If each invocation uses too many registers → fewer warps → lower occupancy → exposed latency. Check with GPU profiling tools.

---

## Phase 6 — Comparative Profiling & Optimization

1. **CPU vs GPU**: render the same scene on both paths. Measure wall-clock time, throughput (Mpixels/sec), and energy.
2. **CPU deep profiling**:
   - `perf stat` — IPC, cache miss rates at each level, branch miss rate, TLB miss rate
   - `perf record` + `perf report` — hotspot analysis, identify the bottleneck function
   - `perf c2c` — detect false sharing
   - `toplev` (pmu-tools) — top-down microarchitectural analysis: frontend-bound, backend-bound (memory vs core), bad speculation, retiring
   - On Apple Silicon: Instruments → CPU Counters template
3. **Experiment matrix** (each is a before/after measurement):
   - AoS vs SoA normal storage
   - Tile size: 32², 64², 128², 256²
   - Z-order (Morton) vs row-major tile layout
   - Branchless vs branchy shadow computation
   - Software prefetch vs hardware-only
   - Huge pages vs 4 KB pages
   - SIMD width: scalar vs 128-bit vs 256-bit (vs 512-bit if available)
   - Thread count: 1, 2, 4, 8, N
   - Ray packet size: 1, 4, 8, 16
4. **Build a results table**: for each experiment, record cycles, IPC, L1/L2/L3 miss rates, and wall time. This becomes your personal reference for "what actually matters."

---

## Phase 7 — Stretch Goals

- **Out-of-core streaming**: extend to 10k+ × 10k+ data that doesn't fit in RAM. Memory-mapped tiles with `mmap` + `madvise`. Measure page faults vs explicit I/O.
- **Ambient occlusion**: cast many shadow rays per pixel (hemisphere sampling). Massively parallel → great SIMD/GPU exercise.
- **Animation**: sweep the sun angle over a day, render frames, output video. Tests sustained throughput.
- **ARM vs x86 comparison**: run the same binary on both platforms, compare microarch behavior (wider ROB on Apple Silicon vs higher clock on x86).

---

## Verification

- **Correctness**: compare CPU and GPU rendered output pixel-by-pixel (allow ±1 due to float precision).
- **Performance**: each phase should include before/after profile numbers. The experiment matrix (Phase 6) is the main deliverable beyond the visual output.
- **Visual**: the final output should be a recognizable shaded mountain terrain with realistic sun shadows — you'll know the math is right because it *looks* right.

---

## Decisions

- **Rust** as language — zero-cost abstractions, `std::arch` SIMD intrinsics, `rayon` for parallelism, `wgpu` for GPU.
- **Full hardware model** depth — every phase explicitly reasons about store buffers, ROB, TLB, branch predictors, not just "use SIMD."
- **Both x86 + ARM** — design SIMD abstraction layer, profile on both.
- **CPU + GPU paths** — CPU raymarcher for deep microarch exploration, GPU compute for comparison of execution models.
- **Medium data (~4000×4000)** — large enough to stress L2/L3 without requiring out-of-core (that's a stretch goal).
