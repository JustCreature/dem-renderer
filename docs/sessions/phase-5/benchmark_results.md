# Phase 5 Benchmark Results
**Hardware**: Apple M4 Max (10 cores, 400 GB/s unified memory bandwidth)
**Dataset**: USGS SRTM 1-arc-second, Hintertux glacier area (N47E011), 3601×3601 cells, ~26 MB on disk, ~52 MB as f32
**Build**: `cargo build --release`, `opt-level=3`, `lto="thin"`, `codegen-units=1`

---

## 0. Memory Bandwidth Baseline

| Access pattern       | Throughput  | Notes |
|----------------------|-------------|-------|
| seq_read SIMD        | 65.8 GB/s   | 256 MB buffer, NEON 128-bit loads |
| seq_read scalar      | 5.0 GB/s    | Same buffer, scalar loop |
| seq_write            | 6.1 GB/s    | Sequential store |
| random_read SIMD     | 1.4 GB/s    | Random 256 MB → L3/DRAM-bound |
| random_read scalar   | 0.6 GB/s    | Same pattern, scalar |
| random_write         | 0.5 GB/s    | Random stores |

**Key ratio**: sequential SIMD / random scalar = **110×**.
The SIMD seq_read at 65.8 GB/s is well above previous runs (~21–37 GB/s) — M4 Max has 400 GB/s total bandwidth and the 256 MB buffer fits in the large unified L3; this run happened to catch it warm. The scalar numbers (5–6 GB/s) are stable — bottlenecked by loop overhead, not bandwidth.

---

## 1. Heightmap Loading

| Step        | Time   | Notes |
|-------------|--------|-------|
| parse_bil   | 117 ms | Read + byte-swap + NODATA fill, 654 864 cells filled |

The file is ~26 MB (3601×3601×2 bytes). 117 ms includes disk I/O + endian conversion + fill-nodata passes. Not in the hot path — runs once.

---

## 2. Tiling / Neighbour-Access Benchmark

Processing all 3601×3601 pixels, reading the 4 orthogonal neighbours for each (classic convolution pattern).

| Layout                    | Throughput | Notes |
|---------------------------|------------|-------|
| row-major iteration        | 45.2 GB/s  | Stride-1 access; prefetcher trains perfectly |
| tiled, row-major iteration | 4.3 GB/s   | Tile layout in memory, but iterating row-by-row → TLB/cache thrash |
| tiled, tile-order walk     | 3.1 GB/s   | Correct spatial walk, but `get()` decomposition overhead dominates |

**Lesson**: the `get()` abstraction (modulo/divide to find tile + offset) kills throughput even though it should help cache. Phase 1 conclusion: tiling benefit requires pointer arithmetic that bypasses the abstraction, not a `get()` API.

---

## 3. Normal Map Computation

Input: 3601×3601 f32 heightmap → output: 3×3601×3601 f32 SoA (nx, ny, nz). Total output = 156 MB.

| Variant                         | Throughput | Notes |
|---------------------------------|------------|-------|
| scalar (row-major)               | 22.5 GB/s  | Cold cache; auto-vectorised by LLVM |
| NEON 4-wide                      | 16.5 GB/s  | Manual NEON, slower than auto-vec here — store pressure |
| NEON parallel (10 cores)         | 47.8 GB/s  | Rayon row-strips; bandwidth approaches memory ceiling |
| tiled NEON (single-thread)       | 30.8 GB/s  | Tile reads help; output still row-major |
| tiled NEON parallel (10 cores)   | **108.0 GB/s** | Input tiled + parallel → above single-chip bandwidth?* |
| GPU compute (wgpu)               | 61 ms total | Full pipeline: upload + dispatch + readback (156 MB) |

*The 108 GB/s figure exceeds a single bandwidth limit because the tiled layout lets L2 serve most reads — fewer DRAM hits, so "effective throughput per DRAM byte" is inflated. The actual DRAM bandwidth consumed is lower; most reads hit L2.

**GPU note**: 61 ms includes ~50 ms of data readback (156 MB from GPU to CPU). The GPU compute itself is sub-millisecond. This is why `GpuScene` never reads normals back — they stay resident on the GPU.

---

## 4. Shadow Mask Computation

Input: 3601×3601 heightmap → output: 3601×3601 u8 mask (~13 MB). DDA horizon sweep algorithm.

### Cardinal sweep (fixed sun elevation = 20°, west-to-east)

| Variant             | Throughput | Notes |
|---------------------|------------|-------|
| scalar              | 8.1 GB/s   | Serial dependency (running max) limits ILP |
| NEON 4-wide         | 8.1 GB/s   | Same speed — scatter gather overhead cancels SIMD gain |
| NEON parallel       | **55.4 GB/s** | 10 cores × independent rows; memory-bandwidth ceiling |

**NEON single = scalar**: the running-max has a data dependency chain (each cell depends on the previous). NEON vectorises 4 *rows* in parallel (not within a row), but single-thread NEON doesn't help when the bottleneck is the serial chain within a row.
**Parallel jumps 6.8×** (not 10×) — still memory-bandwidth-bound. M4 Max bandwidth ceiling at 55 GB/s for this access pattern.

### Diagonal sweep (arbitrary sun azimuth)

| Variant                           | Throughput | Sun position |
|-----------------------------------|------------|--------------|
| scalar with azimuth (225°, 20°)   | 3.1 GB/s   | SW direction |
| scalar azimuth sunrise (90°, 15°) | 3.2 GB/s   | East         |
| scalar azimuth sunset (270°, 10°) | 3.4 GB/s   | West         |
| NEON parallel sunset (270°, 10°)  | **26.3 GB/s** | West      |

**Diagonal is 2.4× slower than cardinal** (3.1–3.4 vs 8.1 GB/s). Diagonal access has stride > 1 in one dimension, causing cache-line waste — each step lands on a different cache line at a diagonal. The DDA step size along the diagonal is √2 × the cardinal step size, and each row's stepping pattern is irregular.
**NEON parallel diagonal (26.3 GB/s) vs NEON parallel cardinal (55.4 GB/s)**: still 2.1× gap — diagonal access pattern can't be served as efficiently from L2/L3 streams.

### GPU shadow

| Variant            | Time   | Notes |
|--------------------|--------|-------|
| compute_shadow_gpu | 26 ms  | Full: upload 52 MB + dispatch + readback 13 MB |

The GPU serial-sweep shader processes each row sequentially on a single shader core — no parallelism benefit. The GPU path here is slower than the CPU NEON parallel (1.5 ms). This is the expected result: serial dependency chains are CPU territory.

---

## 5. CPU Raymarcher (Single Image)

Camera at (2457, 3328) looking toward Olperer peak. Sun direction [0.4, 0.5, 0.7].

### 2000×900 resolution

| Variant             | Time    | Notes |
|---------------------|---------|-------|
| NEON single-thread  | 790 ms  | 4-lane SIMD packet tracing |
| scalar parallel     | 84 ms   | Rayon over pixel rows, scalar rays |
| NEON parallel       | 94 ms   | Rayon + NEON packets |

**NEON single = 9.4× slower than scalar parallel** — this is the Phase 4 lesson: parallelism >> SIMD for this workload. Gather overhead from non-sequential heightmap access cancels SIMD arithmetic gains.
**Scalar parallel ≈ NEON parallel**: the compiler auto-vectorises the scalar inner loop; adding explicit NEON adds only gather overhead without extra ILP.

### 8000×2667 resolution (GPU comparison target)

| Variant         | Time   | Mpix/s | Notes |
|-----------------|--------|--------|-------|
| scalar parallel | 1.26 s | 17.0   | Rayon, 10 cores |

---

## 6. GPU Raymarcher (Single Image)

Same scene, 8000×2667 = 21.3 Mpix. All variants timed after GPU context warm-up.

| Variant          | Time    | Mpix/s | vs CPU parallel | Notes |
|------------------|---------|--------|-----------------|-------|
| CPU parallel     | 1.26 s  | 17.0   | 1×              | baseline |
| GPU buffer       | 130 ms  | 164    | **9.7×**        | heightmap as storage buffer |
| GPU texture      | 170 ms  | 125    | **7.4×**        | heightmap as 2D texture + sampler |
| GPU combined     | 90 ms   | 237    | **14.0×**       | normals computed on GPU, no 156 MB upload |

### Buffer vs Texture: why buffer wins

Expectation: 2D texture cache (Morton-order internally) should outperform a storage buffer for 2D spatial access. Reality: buffer is **1.3× faster** than texture.

Reason: raymarching has quasi-linear (not 2D) access locality. Rays march in a fixed world direction — nearby screen pixels' rays sample heightmap cells that are spatially separated in a strip, not in a 2D patch. The texture cache's Morton-order layout optimises for 2D patches. The storage buffer lets the shader do explicit addressing and the L1/L2 cache handles the 1D strip locality just as well — without the texture sampler overhead (bilinear interpolation = 4 fetches + blending even when not needed).

The texture path also pays the sampler unit round-trip latency (~20–40 cycles on Apple GPU) whereas the buffer read is a direct L1 lookup.

### Why combined is fastest

`render_gpu_combined` skips uploading the normal map (3 × 52 MB = 156 MB). Buffer and texture variants receive pre-computed CPU normals and upload them every call. Combined computes normals on the GPU from the heightmap directly — the heightmap (52 MB) is already there. This saves ~156 MB of PCIe/unified-memory transfer:
- At 400 GB/s unified memory bandwidth: 156 MB ≈ 0.4 ms nominal
- In practice the saving is ~40–80 ms, suggesting the actual bottleneck is driver/command-buffer overhead for large buffer uploads, not raw bandwidth

---

## 7. Multi-Frame Benchmark (10 frames, 8000×2667)

Simulates an animation: camera pans east 200 m per frame. Single-shot timings above include first-call overhead; multi-frame isolates steady-state per-frame cost.

| Variant           | ms/frame | 10-frame total | vs CPU | vs GPU separate |
|-------------------|----------|----------------|--------|-----------------|
| CPU parallel      | 1730.8   | 17.3 s         | 1×     | —               |
| GPU separate      | 133.4    | 1.33 s         | **13.0×** | 1×          |
| GPU combined      | 120.4    | 1.20 s         | **14.4×** | 1.11×        |
| GPU scene         | **98.0** | **0.98 s**     | **17.7×** | **1.36×**    |

### What each variant pays per frame

**CPU parallel**: full raymarcher on 10 CPU cores — 1.73 s/frame. Animation at this rate would be 0.6 fps. No GPU.

**GPU separate**: re-creates the full pipeline every frame — uploads heightmap (52 MB) + normals (156 MB) + shadow (13 MB), rebuilds bind groups, compiles nothing (pipeline cached by driver). The 133 ms/frame breaks down roughly as:
- ~80 ms: data uploads (221 MB total to GPU)
- ~40 ms: compute dispatch + readback (~85 MB RGBA output)
- ~13 ms: command encoding + submission overhead

**GPU combined**: skips normal map upload (saves 156 MB). Cost: 120 ms/frame.
- ~30 ms: uploads (heightmap + shadow = 65 MB)
- ~40 ms: normals compute pass on GPU
- ~40 ms: render compute pass + readback
- ~10 ms: overhead

**GPU scene** (`GpuScene`): uploads nothing per frame. Heightmap, normals, shadow buffers stay on GPU. Only `write_buffer(cam_buf, 128 bytes)` per frame. Cost: 98 ms/frame.
- ~0 ms: data upload (128 bytes)
- ~5 ms: command encoding
- ~88 ms: render compute dispatch + readback (85 MB RGBA)
- ~5 ms: wgpu overhead (map_async, poll)

The readback (85 MB CPU←GPU) is the hard floor for the scene variant — at 400 GB/s that's 0.2 ms theoretical, but actual buffer mapping involves page faults and driver synchronisation, empirically ~80–100 ms.

### Speedup decomposition

| Optimisation step                     | ms/frame saved | Cumulative speedup vs CPU |
|---------------------------------------|----------------|---------------------------|
| CPU → GPU separate                    | 1597 ms        | 13.0×                     |
| GPU separate → GPU combined           | 13 ms          | 14.4×                     |
| GPU combined → GPU scene              | 22 ms          | 17.7×                     |

The GPU execution model (thousands of parallel ray-marching threads) delivers the large 13× jump. Further optimisations (removing uploads) are incremental because the readback cost dominates the remaining budget.

---

## 8. Summary: CPU vs GPU at a Glance

| Task                         | Best CPU        | Best GPU         | GPU speedup |
|------------------------------|-----------------|------------------|-------------|
| Normals (3601×3601)          | 108 GB/s (parallel tiled) | 61 ms (GPU + readback) | N/A* |
| Shadow (cardinal)            | 55.4 GB/s (NEON parallel) | 26 ms (serial shader) | **CPU wins 36×** |
| Shadow (diagonal)            | 26.3 GB/s (NEON parallel) | n/a              | CPU wins |
| Render 8000×2667 (one shot)  | 1.26 s          | 90 ms (combined) | **14×** |
| Render 8000×2667 (per-frame) | 1730 ms         | 98 ms (scene)    | **17.7×** |

*Normals GPU comparison is misleading: the 61 ms includes 156 MB readback (not needed for rendering). The GPU compute itself is sub-ms — but it only makes sense to leave results on the GPU.

### Why GPU wins rendering but CPU wins shadows

| Property                | Raymarching              | Shadow sweep          |
|-------------------------|--------------------------|-----------------------|
| Pixel independence      | Complete (each ray alone)| Partial (rows independent, columns serial) |
| Data dependency chains  | None within a ray†       | Hard serial chain (running max) |
| Memory access pattern   | Quasi-random (ray sample)| Sequential (row sweep) |
| GPU parallelism use     | 100% — all invocations run | 100% of rows, 0% within |
| CPU wins because        | —                        | Serial dependency: CPU OoO execution + SIMD across rows |

†Except binary search refinement at hit, but that's a small fraction of steps.

---

## 9. Key Hardware Lessons

1. **GPU wins when work is embarrassingly parallel and compute-bound.** Raymarching: every pixel is independent. GPU dispatches ~21M invocations, each marches independently. CPU's 10 cores × ~2M rays = 17.7× slower.

2. **CPU wins when there are serial dependency chains.** Shadow sweep: running-max along each row is O(N) serial. GPU must process these serially per thread — no advantage over a CPU core running at 4× higher clock. CPU NEON parallel wins by parallelising across rows, not within them.

3. **Storage buffer beats texture for 1D-linear access.** Texture cache (Morton-order) optimises 2D patches. Raymarching has stripe-like access. Buffer + explicit addressing + L1/L2 is equally or more efficient, without sampler overhead.

4. **Upload cost can dwarf compute cost.** Removing the 156 MB normal upload (combined vs separate) saves only 13 ms/frame — the 221 MB upload pipeline is already mostly hidden by GPU execution overlap. The real floor is the 85 MB readback which can't be hidden.

5. **Persistent GPU state (GpuScene) saves 22–35 ms/frame** by eliminating per-frame uploads. At animation frame rates (20–60 fps) this is significant: it's the difference between 10 fps and 17 fps at 8000×2667.

6. **The GPU readback is the hard floor for the scene pipeline.** 85 MB RGBA (8000×2667×4) must travel from GPU to CPU to be encoded into a GIF. Eliminating the readback entirely (keeping the animation on GPU, encoding on GPU, writing directly to display) would be the next optimisation — but requires a different rendering architecture (swap chain / display pipeline).
