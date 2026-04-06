// src/benchmarks/phase6.rs
//
// Phase 6 — Experiment Matrix
// Experiment 1: Tile size sweep (T = 32, 64, 128, 256)
//
// Kernel: 5-point stencil sum (synthetic — isolates memory access pattern;
//         no SIMD, no normalise, no branch complexity)
// Input:  i16 heightmap  |  Output: f32 row-major buffer
// Variants:
//   - row-major input, row-major iteration (baseline)
//   - tiled input, tile-by-tile iteration (T = 32 / 64 / 128 / 256)
//
// Bytes per pixel: 5 × i16 reads (10 B) + 1 × f32 write (4 B) = 14 B
// Border pixels (global row/col 0 and last) are skipped in all variants.

use crate::utils::*;
use dem_io::{Heightmap, TiledHeightmap};
use rayon::prelude::*;

fn evict_cache() {
    // ~100 MB eviction — flushes L1/L2/L3 before each timed run
    let evict: Vec<i32> = (0..25 * 1024 * 1024).map(|i| i as i32).collect();
    std::hint::black_box(evict);
}

// Row-major baseline.
// All 5 neighbours addressed via flat index into hm.data (stride = cols).
// North/south neighbours are `cols` elements apart — ~14 KB at 3601 cols.
#[inline(never)]
fn stencil_rowmajor(hm: &Heightmap, output: &mut [f32]) {
    let rows = hm.rows;
    let cols = hm.cols;
    let data = &hm.data;
    for r in 1..rows - 1 {
        for c in 1..cols - 1 {
            output[r * cols + c] = data[r * cols + c] as f32
                + data[(r - 1) * cols + c] as f32
                + data[(r + 1) * cols + c] as f32
                + data[r * cols + c - 1] as f32
                + data[r * cols + c + 1] as f32;
        }
    }
}

// Tiled variant — tile-by-tile iteration, direct pointer arithmetic, no get().
//
// For interior pixels (r > 0, r < ts-1, c > 0, c < ts-1 within the tile) all 5
// neighbours are in the same tile → no cross-tile addressing.
// For tile-border pixels (~6% of work at T=64), halo is resolved via precomputed
// neighbour tile base offsets. Branches are well-predicted (false ~94% of the time).
#[inline(never)]
fn stencil_tiled(hm: &TiledHeightmap, output: &mut [f32]) {
    let ts = hm.tile_size;
    let ts2 = ts * ts;
    let tile_rows = hm.tile_rows;
    let tile_cols = hm.tile_cols;
    let tiles = hm.tiles();

    for tr in 0..tile_rows {
        for tc in 0..tile_cols {
            let tile_base = (tr * tile_cols + tc) * ts2;

            // Neighbour tile bases — computed once per tile, used only for halo pixels
            let tile_n = if tr > 0 {
                ((tr - 1) * tile_cols + tc) * ts2
            } else {
                tile_base
            };
            let tile_s = if tr + 1 < tile_rows {
                ((tr + 1) * tile_cols + tc) * ts2
            } else {
                tile_base
            };
            let tile_w = if tc > 0 {
                (tr * tile_cols + tc - 1) * ts2
            } else {
                tile_base
            };
            let tile_e = if tc + 1 < tile_cols {
                (tr * tile_cols + tc + 1) * ts2
            } else {
                tile_base
            };

            for r in 0..ts {
                for c in 0..ts {
                    let global_r = tr * ts + r;
                    let global_c = tc * ts + c;

                    if global_r == 0
                        || global_r >= hm.rows - 1
                        || global_c == 0
                        || global_c >= hm.cols - 1
                    {
                        continue;
                    }

                    let center = tiles[tile_base + r * ts + c] as f32;

                    let north = if r > 0 {
                        tiles[tile_base + (r - 1) * ts + c]
                    } else {
                        tiles[tile_n + (ts - 1) * ts + c]
                    } as f32;

                    let south = if r + 1 < ts {
                        tiles[tile_base + (r + 1) * ts + c]
                    } else {
                        tiles[tile_s + c]
                    } as f32;

                    let west = if c > 0 {
                        tiles[tile_base + r * ts + c - 1]
                    } else {
                        tiles[tile_w + r * ts + ts - 1]
                    } as f32;

                    let east = if c + 1 < ts {
                        tiles[tile_base + r * ts + c + 1]
                    } else {
                        tiles[tile_e + r * ts]
                    } as f32;

                    output[global_r * hm.cols + global_c] = center + north + south + west + east;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Experiment 5: Morton (Z-order) vs row-major tile ordering
// ---------------------------------------------------------------------------
//
// Both layouts store pixels in row-major order WITHIN each tile.
// The difference is how tiles are ordered in the flat buffer:
//   row-major tiles:  tile(tr,tc) at (tr * tile_cols + tc) * ts²
//   Morton tiles:     tile(tr,tc) at morton2(tc, tr) * ts²
//
// For the 5-point stencil, the N/S neighbour tile is:
//   row-major: tile_cols tiles away → tile_cols × ts² × 2 bytes
//   Morton:    typically 1-4 positions away in Z-curve → ~ts² × 8 bytes
//
// Hypothesis: if the N/S tile distance in row-major is causing cache misses,
// Morton should help. If the bottleneck is purely scalar (the `continue` branch
// preventing vectorisation, as shown in Exp 1), Morton won't help at all.

fn next_pow2(n: usize) -> usize {
    let mut p = 1;
    while p < n {
        p <<= 1;
    }
    p
}

fn spread_bits(v: u32) -> u32 {
    let mut v = v & 0x0000_FFFF;
    v = (v | (v << 8)) & 0x00FF_00FF;
    v = (v | (v << 4)) & 0x0F0F_0F0F;
    v = (v | (v << 2)) & 0x3333_3333;
    v = (v | (v << 1)) & 0x5555_5555;
    v
}

#[inline(always)]
fn morton2(x: u32, y: u32) -> u32 {
    spread_bits(x) | (spread_bits(y) << 1)
}

struct MortonHeightmap {
    data: Vec<i16>,
    rows: usize,
    cols: usize,
    tile_size: usize,
    tile_rows: usize,
    tile_cols: usize,
    dim: usize, // next power-of-2 >= max(tile_rows, tile_cols)
}

fn build_morton_heightmap(hm: &Heightmap, tile_size: usize) -> MortonHeightmap {
    let tile_rows = (hm.rows + tile_size - 1) / tile_size;
    let tile_cols = (hm.cols + tile_size - 1) / tile_size;
    let dim = next_pow2(tile_rows.max(tile_cols));
    let ts2 = tile_size * tile_size;
    let mut data = vec![0i16; dim * dim * ts2];

    for tr in 0..tile_rows {
        for tc in 0..tile_cols {
            let tile_base = morton2(tc as u32, tr as u32) as usize * ts2;
            for r in 0..tile_size {
                for c in 0..tile_size {
                    let src_r = (tr * tile_size + r).min(hm.rows - 1);
                    let src_c = (tc * tile_size + c).min(hm.cols - 1);
                    data[tile_base + r * tile_size + c] = hm.data[src_r * hm.cols + src_c];
                }
            }
        }
    }

    MortonHeightmap {
        data,
        rows: hm.rows,
        cols: hm.cols,
        tile_size,
        tile_rows,
        tile_cols,
        dim,
    }
}

#[inline(never)]
fn stencil_morton(hm: &MortonHeightmap, output: &mut [f32]) {
    let ts = hm.tile_size;
    let ts2 = ts * ts;
    let tile_rows = hm.tile_rows;
    let tile_cols = hm.tile_cols;
    let data = &hm.data;

    for tr in 0..tile_rows {
        for tc in 0..tile_cols {
            let tile_base = morton2(tc as u32, tr as u32) as usize * ts2;

            let tile_n = if tr > 0 {
                morton2(tc as u32, (tr - 1) as u32) as usize * ts2
            } else {
                tile_base
            };
            let tile_s = if tr + 1 < tile_rows {
                morton2(tc as u32, (tr + 1) as u32) as usize * ts2
            } else {
                tile_base
            };
            let tile_w = if tc > 0 {
                morton2((tc - 1) as u32, tr as u32) as usize * ts2
            } else {
                tile_base
            };
            let tile_e = if tc + 1 < tile_cols {
                morton2((tc + 1) as u32, tr as u32) as usize * ts2
            } else {
                tile_base
            };

            for r in 0..ts {
                for c in 0..ts {
                    let global_r = tr * ts + r;
                    let global_c = tc * ts + c;
                    if global_r == 0
                        || global_r >= hm.rows - 1
                        || global_c == 0
                        || global_c >= hm.cols - 1
                    {
                        continue;
                    }

                    let center = data[tile_base + r * ts + c] as f32;
                    let north = if r > 0 {
                        data[tile_base + (r - 1) * ts + c]
                    } else {
                        data[tile_n + (ts - 1) * ts + c]
                    } as f32;
                    let south = if r + 1 < ts {
                        data[tile_base + (r + 1) * ts + c]
                    } else {
                        data[tile_s + c]
                    } as f32;
                    let west = if c > 0 {
                        data[tile_base + r * ts + c - 1]
                    } else {
                        data[tile_w + r * ts + ts - 1]
                    } as f32;
                    let east = if c + 1 < ts {
                        data[tile_base + r * ts + c + 1]
                    } else {
                        data[tile_e + r * ts]
                    } as f32;

                    output[global_r * hm.cols + global_c] = center + north + south + west + east;
                }
            }
        }
    }
}

pub(crate) fn bench_morton_vs_rowmajor(hm: &Heightmap) {
    let rows = hm.rows;
    let cols = hm.cols;
    let pixels = (rows - 2) * (cols - 2);
    let bytes_per_run = pixels * 14;

    let tile_size = 64usize;
    let tiled = TiledHeightmap::from_heightmap(hm, tile_size);
    let morton = build_morton_heightmap(hm, tile_size);

    let tile_rows = tiled.tile_rows;
    let tile_cols = tiled.tile_cols;
    let dim = morton.dim;
    let tile_kb = tile_size * tile_size * 2 / 1024;

    println!("--- Phase 6 / Exp 5: Morton vs Row-major Tile Ordering ---");
    println!(
        "  tile_size={}, {}×{} tiles  |  row-major: {} MB  |  Morton (dim={}): {} MB (+{}% overhead)",
        tile_size,
        tile_rows,
        tile_cols,
        tile_rows * tile_cols * tile_size * tile_size * 2 / 1_000_000,
        dim,
        dim * dim * tile_size * tile_size * 2 / 1_000_000,
        (dim * dim * 100 / (tile_rows * tile_cols)) - 100,
    );
    println!(
        "  N/S tile gap — row-major: {} tiles = {} KB  |  Morton: 1–4 tiles = {}–{} KB",
        tile_cols,
        tile_cols * tile_kb,
        tile_kb,
        4 * tile_kb
    );
    println!("  M4 Max L1D=128 KB, L2=16 MB — row-major N/S gap fits in L2 but not L1");
    println!();

    let mut output = vec![0.0f32; rows * cols];

    evict_cache();
    let (ticks, _) = profiling::timed("stencil_rowmajor", || {
        stencil_rowmajor(hm, &mut output);
        std::hint::black_box(&output);
    });
    println!(
        "  row-major (vectorised baseline): {:>6.2} GB/s",
        count_gb_per_sec(ticks, Some(bytes_per_run))
    );

    evict_cache();
    let (ticks, _) = profiling::timed("stencil_tiled_rowmajor", || {
        stencil_tiled(&tiled, &mut output);
        std::hint::black_box(&output);
    });
    let tiled_gbs = count_gb_per_sec(ticks, Some(bytes_per_run));
    println!(
        "  tiled row-major T={:<3}:          {:>6.2} GB/s",
        tile_size, tiled_gbs
    );

    evict_cache();
    let (ticks, _) = profiling::timed("stencil_morton", || {
        stencil_morton(&morton, &mut output);
        std::hint::black_box(&output);
    });
    let morton_gbs = count_gb_per_sec(ticks, Some(bytes_per_run));
    println!(
        "  tiled Morton    T={:<3}:          {:>6.2} GB/s  ({:.2}× vs row-major tiled)",
        tile_size,
        morton_gbs,
        morton_gbs / tiled_gbs
    );
    println!();
    println!("  if Morton ≈ row-major tiled: bottleneck is scalar loop (continue branch),");
    println!("    not N/S tile cache distance — layout is irrelevant at this throughput");
    println!("  if Morton > row-major tiled: N/S cache miss was a real contributor");
}

pub(crate) fn bench_thread_count_scaling(hm: &Heightmap) {
    let rows = hm.rows;
    let cols = hm.cols;
    let pixels = (rows - 2) * (cols - 2);
    let bytes_per_run = pixels * 14; // 5 × i16 reads + 1 × f32 write

    println!("--- Phase 6 / Exp 2: Thread Count Scaling ---");
    println!("  kernel: row-major stencil (vectorised baseline), cold cache");
    println!("  M4 Max: 12 perf cores, ~400 GB/s peak bandwidth");
    println!();

    for &n_threads in &[1usize, 2, 4, 6, 8, 10, 12] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(n_threads)
            .build()
            .unwrap();

        let mut output = vec![0.0f32; rows * cols];
        evict_cache();

        let data = &hm.data;
        let ticks = pool.install(|| {
            let (ticks, _) = profiling::timed("stencil_parallel", || {
                output
                    .par_chunks_mut(cols)
                    .enumerate()
                    .skip(1)
                    .take(rows - 2)
                    .for_each(|(r, row_out)| {
                        for c in 1..cols - 1 {
                            row_out[c] = data[r * cols + c] as f32
                                + data[(r - 1) * cols + c] as f32
                                + data[(r + 1) * cols + c] as f32
                                + data[r * cols + c - 1] as f32
                                + data[r * cols + c + 1] as f32;
                        }
                    });
                std::hint::black_box(&output);
            });
            ticks
        });

        let gb_s = count_gb_per_sec(ticks, Some(bytes_per_run));
        let single = 60.22f64; // row-major single-thread baseline from Exp 1
        println!(
            "  {:>2} thread{}: {:>6.1} GB/s  ({:.1}× vs single)",
            n_threads,
            if n_threads == 1 { " " } else { "s" },
            gb_s,
            gb_s / single,
        );
    }
    println!();
    println!("  bandwidth ceiling: ~400 GB/s total / 14 bytes/pixel ≈ 28 Gpix/s theoretical");
    println!("  single-thread ceiling: ~60 GB/s / 14 bytes ≈ 4.3 Gpix/s");
}

pub(crate) fn bench_thread_count_scaling_readonly(hm: &Heightmap) {
    let rows = hm.rows;
    let cols = hm.cols;
    let pixels = (rows - 2) * (cols - 2);
    let bytes_per_run = pixels * 10; // 5 × i16 reads only, no write

    println!("--- Phase 6 / Exp 3: Thread Scaling (read-only, no output write) ---");
    println!("  hypothesis: flattening at 8+ threads is write-path pressure, not DRAM BW");
    println!();

    let data = &hm.data;

    for &n_threads in &[1usize, 2, 4, 6, 8, 10, 12] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(n_threads)
            .build()
            .unwrap();

        evict_cache();

        let ticks = pool.install(|| {
            let (ticks, _) = profiling::timed("stencil_readonly", || {
                let _sum: i64 = (1..rows - 1)
                    .into_par_iter()
                    .map(|r| {
                        let mut row_sum = 0i64;
                        for c in 1..cols - 1 {
                            row_sum += data[r * cols + c] as i64
                                + data[(r - 1) * cols + c] as i64
                                + data[(r + 1) * cols + c] as i64
                                + data[r * cols + c - 1] as i64
                                + data[r * cols + c + 1] as i64;
                        }
                        row_sum
                    })
                    .sum();
                std::hint::black_box(_sum);
            });
            ticks
        });

        let gb_s = count_gb_per_sec(ticks, Some(bytes_per_run));
        let single_readonly = 11.5f64; // 1-thread from Exp 2 (scalar closure, no write)
        println!(
            "  {:>2} thread{}: {:>6.1} GB/s  ({:.1}× vs 1-thread)",
            n_threads,
            if n_threads == 1 { " " } else { "s" },
            gb_s,
            gb_s / single_readonly,
        );
    }
    println!();
    println!("  if write-path is the bottleneck: scaling should continue past 8 threads");
    println!("  if DRAM BW is the bottleneck: scaling flattens at same point as Exp 2");
}

// ---------------------------------------------------------------------------
// Experiment 8: Ray packet gather cost
// ---------------------------------------------------------------------------
//
// The Phase 4 mystery: scalar single-thread = NEON 4-ray single-thread (both 0.80s).
// Root cause hypothesis: gather overhead cancels the 4-wide SIMD gain.
//
// This experiment measures the heightmap lookup cost at different access patterns:
//
//   1-wide:           1 random lookup/step           → ~1 cache miss/step
//   4-wide adjacent:  4 lookups at [p, p+1, p+2, p+3] → 1 miss/step (same cache line!)
//   4-wide strided:   4 lookups at [p, p+cols, ..  ]   → 4 misses/step (different rows)
//   4-wide random:    4 fully independent lookups       → 4 misses/step (worst case)
//
// Ray packets start adjacent (pixels next to each other on screen), then diverge
// as rays travel at different speeds through varying terrain. The NEON packet
// spends most steps in the "strided/random" regime — this is the gather cost.

#[inline(never)]
fn gather_1wide(data: &[i16], positions: &[usize]) -> i64 {
    let mut sum = 0i64;
    for &p in positions {
        sum += data[p] as i64;
    }
    sum
}

#[inline(never)]
fn gather_4wide_adjacent(data: &[i16], positions: &[usize]) -> i64 {
    // Best case: 4 pixels at p, p+1, p+2, p+3 — all within the same cache line
    // (4 × i16 = 8 bytes; a 64-byte line holds 32 i16s)
    let mut sum = 0i64;
    for &p in positions {
        sum += data[p] as i64 + data[p + 1] as i64 + data[p + 2] as i64 + data[p + 3] as i64;
    }
    sum
}

#[inline(never)]
fn gather_4wide_strided(data: &[i16], positions: &[usize], cols: usize) -> i64 {
    // Diverged rows: p, p+cols, p+2*cols, p+3*cols — each on a different row
    // Row stride = cols * 2 bytes = 3601 * 2 = 7202 bytes → always a separate cache line
    let mut sum = 0i64;
    for &p in positions {
        sum += data[p] as i64
            + data[p + cols] as i64
            + data[p + 2 * cols] as i64
            + data[p + 3 * cols] as i64;
    }
    sum
}

#[inline(never)]
fn gather_4wide_random(data: &[i16], pos4: &[[usize; 4]]) -> i64 {
    // 4 fully random positions per step — maximum divergence
    let mut sum = 0i64;
    for p in pos4 {
        sum += data[p[0]] as i64 + data[p[1]] as i64 + data[p[2]] as i64 + data[p[3]] as i64;
    }
    sum
}

pub(crate) fn bench_gather_ray_packets(hm: &dem_io::Heightmap) {
    let rows = hm.rows;
    let cols = hm.cols;
    let data = &hm.data;
    let n_steps = 2 * 1024 * 1024; // 2M steps

    // Random starting positions (avoid border)
    let positions: Vec<usize> = {
        let mut rng = 0xCAFE_BABE_u64;
        (0..n_steps)
            .map(|_| {
                rng = rng
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                let row = (rng >> 33) as usize % (rows - 6) + 3;
                rng = rng
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                let col = (rng >> 33) as usize % (cols - 6) + 3;
                row * cols + col
            })
            .collect()
    };

    // For 4-wide random: group 4 independent positions per "step"
    let pos4: Vec<[usize; 4]> = positions
        .chunks_exact(4)
        .map(|c| [c[0], c[1], c[2], c[3]])
        .collect();

    let bytes_1 = n_steps * 2; // 1 × i16 per step
    let bytes_4 = n_steps * 2 * 4; // 4 × i16 per step

    println!("--- Phase 6 / Exp 8: Ray Packet Gather Cost ---");
    println!(
        "  {} M random positions in {}×{} heightmap ({} MB)",
        n_steps / 1_000_000,
        rows,
        cols,
        rows * cols * 2 / 1_000_000
    );
    println!("  cache line = 64 bytes = 32 i16 values — adjacent pixels share a line");
    println!(
        "  row stride  = {} × 2 = {} bytes — different rows always on different lines",
        cols,
        cols * 2
    );
    println!();

    evict_cache();
    let (ticks, _) = profiling::timed("gather_1w", || {
        std::hint::black_box(gather_1wide(data, &positions));
    });
    let w1 = count_gb_per_sec(ticks, Some(bytes_1));
    println!("  1-wide  (scalar):        {:>5.2} GB/s  ~1 miss/step", w1);

    evict_cache();
    let (ticks, _) = profiling::timed("gather_4w_adj", || {
        std::hint::black_box(gather_4wide_adjacent(data, &positions));
    });
    let w4a = count_gb_per_sec(ticks, Some(bytes_4));
    println!(
        "  4-wide  adjacent cols:   {:>5.2} GB/s  same cache line → {:.1}× bytes, ~{:.1}× time",
        w4a,
        w4a / w1,
        count_gb_per_sec(ticks, Some(bytes_1)) / w1
    );

    evict_cache();
    let (ticks, _) = profiling::timed("gather_4w_str", || {
        std::hint::black_box(gather_4wide_strided(data, &positions, cols));
    });
    let w4s = count_gb_per_sec(ticks, Some(bytes_4));
    println!(
        "  4-wide  strided rows:    {:>5.2} GB/s  separate lines  → {:.1}× bytes, ~{:.1}× time",
        w4s,
        w4s / w1,
        count_gb_per_sec(ticks, Some(bytes_1)) / w1
    );

    evict_cache();
    let (ticks, _) = profiling::timed("gather_4w_rand", || {
        std::hint::black_box(gather_4wide_random(data, &pos4));
    });
    let w4r = count_gb_per_sec(ticks, Some(bytes_4));
    println!(
        "  4-wide  fully random:    {:>5.2} GB/s  max divergence  → {:.1}× bytes, ~{:.1}× time",
        w4r,
        w4r / w1,
        count_gb_per_sec(ticks, Some(bytes_1)) / w1
    );

    println!();
    println!("  NEON 4-ray packet starts adjacent (screen pixels), diverges after ~10–50 steps");
    println!("  → most of the 506 avg steps/ray are in strided/random regime");
    println!("  → gather cost ≈ 4× per step, SIMD gain ≈ 4×, net: no speedup (Phase 4 confirmed)");
}

// ---------------------------------------------------------------------------
// Experiment 9: TLB working set sweep
// ---------------------------------------------------------------------------
//
// macOS/Apple Silicon uses 16KB base pages (not 4KB like x86/Linux).
// L1 DTLB: ~256 entries × 16KB = 4 MB coverage (typical for M-series big core)
// L2 TLB:  ~3000 entries × 16KB = ~48 MB coverage
//
// Random access to an array of size S:
//   S ≤ 4 MB:  all pages fit in L1 DTLB → no TLB miss overhead
//   S ≤ 48 MB: pages spill to L2 TLB   → small additional latency (~5 cycles)
//   S > 48 MB: L2 TLB miss → page table walk → ~50 cycle penalty per unique page
//
// Compare with L3 cache capacity: M4 Max has ~48+ MB SLC.
// Random access will show TWO knees: one at TLB capacity, one at cache capacity.
// On M4 Max, both knees happen at roughly the same size (~48 MB) — they're entangled.
//
// "Huge pages" (32 MB on M1/M2/M3/M4) would extend L2 TLB reach by 2048× —
// but macOS manages these transparently; no user-space API to force them.

pub(crate) fn bench_tlb_sweep(data: &[f32]) {
    let n_accesses = 1 << 20; // 1M accesses (constant across all sizes)

    println!("--- Phase 6 / Exp 9: TLB Working Set Sweep ---");
    println!("  macOS/M4: 16KB base pages (not 4KB like x86)");
    println!("  L1 DTLB: ~256 entries × 16KB = ~4 MB  |  L2 TLB: ~3000 × 16KB = ~48 MB");
    println!(
        "  {} M random reads per size, 4 bytes each",
        n_accesses / 1_000_000
    );
    println!();

    // Working set sizes in floats
    let sizes: &[(usize, &str)] = &[
        (4_096, "16 KB"),
        (16_384, "64 KB"),
        (65_536, "256 KB"),
        (262_144, "1 MB"),
        (1_048_576, "4 MB"),
        (4_194_304, "16 MB"),
        (16_777_216, "64 MB"),
        (data.len().min(67_108_864), "256 MB"),
    ];

    for &(n_floats, label) in sizes {
        let slice = &data[..n_floats];
        let n_pages_16k = (n_floats * 4 + 16383) / 16384;

        // Generate n_accesses random indices within [0, n_floats)
        let indices: Vec<usize> = {
            let mut rng = 0xDEAD_BEEF_u64;
            (0..n_accesses)
                .map(|_| {
                    rng = rng
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(1_442_695_040_888_963_407);
                    (rng >> 33) as usize % n_floats
                })
                .collect()
        };

        evict_cache();
        let (ticks, _) = profiling::timed("tlb_sweep", || {
            let mut sum = 0.0f32;
            for &i in &indices {
                sum += slice[i];
            }
            std::hint::black_box(sum);
        });

        let gb_s = count_gb_per_sec(ticks, Some(n_accesses * 4));
        let tlb_note = if n_floats * 4 <= 4 * 1024 * 1024 {
            "← L1 DTLB"
        } else if n_floats * 4 <= 48 * 1024 * 1024 {
            "← L2 TLB"
        } else {
            "← TLB miss + page walk"
        };
        println!(
            "  {:>6}  ({:>5} 16KB-pages): {:>5.2} GB/s  {}",
            label, n_pages_16k, gb_s, tlb_note
        );
    }

    println!();
    println!("  heightmap (26 MB): 1625 pages → fits L2 TLB (~48 MB), no full page walks");
    println!("  256 MB array: 16384 pages → exceeds L2 TLB → page walks add ~50 cycles/access");
    println!("  huge pages (2MB on x86, 32MB on M4) would reduce 256 MB to ~128 entries → L1 DTLB");
}

// ---------------------------------------------------------------------------
// Experiment 6: Software prefetch — random access
// ---------------------------------------------------------------------------
//
// Random reads from a 256 MB array (exceeds all cache levels):
//   no prefetch: one outstanding miss at a time → latency-bound at ~0.6 GB/s
//   prfm D ahead: D misses in flight simultaneously → latency hiding
//
// Prefetch throughput model:
//   M4 Max DRAM latency ~80 ns, fill-buffer depth ~20 outstanding misses
//   Theoretical ceiling: 20 misses × 64 B/line / 80 ns ≈ 16 GB/s
//   (actual depends on memory controller depth and request queue size)
//
// prfm pldl1keep — "prefetch for load into L1, keep in cache"
//   issues a non-blocking demand request D cache-lines ahead of current position

#[cfg(target_arch = "aarch64")]
#[inline(always)]
unsafe fn prfm(ptr: *const f32) {
    std::arch::asm!(
        "prfm pldl1keep, [{p}]",
        p = in(reg) ptr,
        options(nostack, preserves_flags, readonly)
    );
}

#[inline(never)]
fn random_noprefetch(data: &[f32], indices: &[usize]) -> f32 {
    let mut sum = 0.0f32;
    for &i in indices {
        sum += data[i];
    }
    sum
}

#[cfg(target_arch = "aarch64")]
#[inline(never)]
fn random_prefetch_d4(data: &[f32], indices: &[usize]) -> f32 {
    let mut sum = 0.0f32;
    let n = indices.len();
    for i in 0..n {
        if i + 4 < n {
            unsafe { prfm(data.as_ptr().add(indices[i + 4])) };
        }
        sum += data[indices[i]];
    }
    sum
}

#[cfg(target_arch = "aarch64")]
#[inline(never)]
fn random_prefetch_d16(data: &[f32], indices: &[usize]) -> f32 {
    let mut sum = 0.0f32;
    let n = indices.len();
    for i in 0..n {
        if i + 16 < n {
            unsafe { prfm(data.as_ptr().add(indices[i + 16])) };
        }
        sum += data[indices[i]];
    }
    sum
}

#[cfg(target_arch = "aarch64")]
#[inline(never)]
fn random_prefetch_d64(data: &[f32], indices: &[usize]) -> f32 {
    let mut sum = 0.0f32;
    let n = indices.len();
    for i in 0..n {
        if i + 64 < n {
            unsafe { prfm(data.as_ptr().add(indices[i + 64])) };
        }
        sum += data[indices[i]];
    }
    sum
}

pub(crate) fn bench_software_prefetch(data: &[f32]) {
    let n = data.len(); // N = 64M floats = 256 MB
    let bytes_per_run = n * 4;

    // Pre-generate shuffled indices once (same pattern for all variants)
    let mut indices: Vec<usize> = (0..n).collect();
    shuffle(&mut indices);

    println!("--- Phase 6 / Exp 6: Software Prefetch (random access) ---");
    println!(
        "  {} M random reads, {} MB array (> all cache levels)",
        n / 1_000_000,
        bytes_per_run / 1_000_000
    );
    println!("  prfm pldl1keep: non-blocking L1 prefetch — keeps D misses in flight");
    println!("  M4 Max: ~80 ns DRAM latency, ~20 fill buffers → ceiling ~16 GB/s");
    println!();

    // Evict and run no-prefetch baseline
    evict_cache();
    let (ticks, _) = profiling::timed("rand_nopf", || {
        std::hint::black_box(random_noprefetch(data, &indices));
    });
    println!(
        "  no prefetch:    {:>5.2} GB/s  (1 miss in flight, latency-bound)",
        count_gb_per_sec(ticks, Some(bytes_per_run))
    );

    #[cfg(target_arch = "aarch64")]
    {
        evict_cache();
        let (ticks, _) = profiling::timed("rand_pf4", || {
            std::hint::black_box(random_prefetch_d4(data, &indices));
        });
        println!(
            "  prefetch D= 4:  {:>5.2} GB/s",
            count_gb_per_sec(ticks, Some(bytes_per_run))
        );

        evict_cache();
        let (ticks, _) = profiling::timed("rand_pf16", || {
            std::hint::black_box(random_prefetch_d16(data, &indices));
        });
        println!(
            "  prefetch D=16:  {:>5.2} GB/s",
            count_gb_per_sec(ticks, Some(bytes_per_run))
        );

        evict_cache();
        let (ticks, _) = profiling::timed("rand_pf64", || {
            std::hint::black_box(random_prefetch_d64(data, &indices));
        });
        println!(
            "  prefetch D=64:  {:>5.2} GB/s",
            count_gb_per_sec(ticks, Some(bytes_per_run))
        );
    }

    println!();
    println!("  sequential ceiling (Exp 1): ~68 GB/s — bandwidth-limited, not latency-limited");
}

// ---------------------------------------------------------------------------
// Experiment 7: NEON multiple accumulators — break the serial dep chain
// ---------------------------------------------------------------------------
//
// Auto-vectorised `shade_soa` (Exp 4): 24 GB/s single-thread — far below the
// ~68 GB/s sequential ceiling. Root cause: serial reduction dep chain.
//   sum += f(i)  →  each fadd waits for previous result (3-cycle NEON latency)
//
// With K independent NEON accumulators (4 f32 each):
//   K=1: still serial across 4-element registers → similar to auto-vec
//   K=4: 4 × 4 = 16 pixels per iteration, 4 independent FMA chains
//        CPU can issue all 4 in parallel → pipeline fully utilised
//   K=8: 32 pixels/iter, diminishing returns (memory bandwidth ceiling hit)
//
// Expected crossover: K=4 → memory-bound at ~68 GB/s, not compute-bound

#[cfg(target_arch = "aarch64")]
#[inline(never)]
#[target_feature(enable = "neon")]
unsafe fn shade_vector_1acc(nx: &[f32], ny: &[f32], nz: &[f32], sun: [f32; 3]) -> f32 {
    use core::arch::aarch64::*;
    let sx = vdupq_n_f32(sun[0]);
    let sy = vdupq_n_f32(sun[1]);
    let sz = vdupq_n_f32(sun[2]);
    let mut acc = vdupq_n_f32(0.0);
    let n = nx.len();
    let (nxp, nyp, nzp) = (nx.as_ptr(), ny.as_ptr(), nz.as_ptr());
    let mut i = 0;
    while i + 4 <= n {
        let vx = vld1q_f32(nxp.add(i));
        let vy = vld1q_f32(nyp.add(i));
        let vz = vld1q_f32(nzp.add(i));
        acc = vfmaq_f32(vfmaq_f32(vfmaq_f32(acc, vx, sx), vy, sy), vz, sz);
        i += 4;
    }
    vaddvq_f32(acc)
}

#[cfg(target_arch = "aarch64")]
#[inline(never)]
#[target_feature(enable = "neon")]
unsafe fn shade_vector_4acc(nx: &[f32], ny: &[f32], nz: &[f32], sun: [f32; 3]) -> f32 {
    use core::arch::aarch64::*;
    let sx = vdupq_n_f32(sun[0]);
    let sy = vdupq_n_f32(sun[1]);
    let sz = vdupq_n_f32(sun[2]);
    let (mut a0, mut a1, mut a2, mut a3) = (
        vdupq_n_f32(0.0),
        vdupq_n_f32(0.0),
        vdupq_n_f32(0.0),
        vdupq_n_f32(0.0),
    );
    let n = nx.len();
    let (nxp, nyp, nzp) = (nx.as_ptr(), ny.as_ptr(), nz.as_ptr());
    let mut i = 0;
    while i + 16 <= n {
        let (x0, x1, x2, x3) = (
            vld1q_f32(nxp.add(i)),
            vld1q_f32(nxp.add(i + 4)),
            vld1q_f32(nxp.add(i + 8)),
            vld1q_f32(nxp.add(i + 12)),
        );
        let (y0, y1, y2, y3) = (
            vld1q_f32(nyp.add(i)),
            vld1q_f32(nyp.add(i + 4)),
            vld1q_f32(nyp.add(i + 8)),
            vld1q_f32(nyp.add(i + 12)),
        );
        let (z0, z1, z2, z3) = (
            vld1q_f32(nzp.add(i)),
            vld1q_f32(nzp.add(i + 4)),
            vld1q_f32(nzp.add(i + 8)),
            vld1q_f32(nzp.add(i + 12)),
        );
        a0 = vfmaq_f32(vfmaq_f32(vfmaq_f32(a0, x0, sx), y0, sy), z0, sz);
        a1 = vfmaq_f32(vfmaq_f32(vfmaq_f32(a1, x1, sx), y1, sy), z1, sz);
        a2 = vfmaq_f32(vfmaq_f32(vfmaq_f32(a2, x2, sx), y2, sy), z2, sz);
        a3 = vfmaq_f32(vfmaq_f32(vfmaq_f32(a3, x3, sx), y3, sy), z3, sz);
        i += 16;
    }
    vaddvq_f32(vaddq_f32(vaddq_f32(a0, a1), vaddq_f32(a2, a3)))
}

#[cfg(target_arch = "aarch64")]
#[inline(never)]
#[target_feature(enable = "neon")]
unsafe fn shade_vector_8acc(nx: &[f32], ny: &[f32], nz: &[f32], sun: [f32; 3]) -> f32 {
    use core::arch::aarch64::*;
    let sx = vdupq_n_f32(sun[0]);
    let sy = vdupq_n_f32(sun[1]);
    let sz = vdupq_n_f32(sun[2]);
    let (mut a0, mut a1, mut a2, mut a3, mut a4, mut a5, mut a6, mut a7) = (
        vdupq_n_f32(0.0),
        vdupq_n_f32(0.0),
        vdupq_n_f32(0.0),
        vdupq_n_f32(0.0),
        vdupq_n_f32(0.0),
        vdupq_n_f32(0.0),
        vdupq_n_f32(0.0),
        vdupq_n_f32(0.0),
    );
    let n = nx.len();
    let (nxp, nyp, nzp) = (nx.as_ptr(), ny.as_ptr(), nz.as_ptr());
    let mut i = 0;
    while i + 32 <= n {
        macro_rules! fma {
            ($a:expr, $off:expr) => {{
                let x = vld1q_f32(nxp.add(i + $off));
                let y = vld1q_f32(nyp.add(i + $off));
                let z = vld1q_f32(nzp.add(i + $off));
                $a = vfmaq_f32(vfmaq_f32(vfmaq_f32($a, x, sx), y, sy), z, sz);
            }};
        }
        fma!(a0, 0);
        fma!(a1, 4);
        fma!(a2, 8);
        fma!(a3, 12);
        fma!(a4, 16);
        fma!(a5, 20);
        fma!(a6, 24);
        fma!(a7, 28);
        i += 32;
    }
    let s01 = vaddq_f32(a0, a1);
    let s23 = vaddq_f32(a2, a3);
    let s45 = vaddq_f32(a4, a5);
    let s67 = vaddq_f32(a6, a7);
    vaddvq_f32(vaddq_f32(vaddq_f32(s01, s23), vaddq_f32(s45, s67)))
}

pub(crate) fn bench_vector_accumulators(nm: &terrain::NormalMap) {
    let n = nm.rows * nm.cols;
    let sun = [0.4f32, 0.5f32, 0.7f32];
    let bytes = n * 12; // 3 × f32/pixel

    println!("--- Phase 6 / Exp 7: NEON Accumulators — break serial dep chain ---");
    println!(
        "  dot(normal, sun) over {} Mpix, {} MB total (3 × SoA arrays)",
        n / 1_000_000,
        bytes / 1_000_000
    );
    println!("  Exp 4 result: auto-vec = 24 GB/s → compute-bound (fadd latency)");
    println!("  This exp: explicit NEON 1/4/8 independent accumulators");
    println!();

    evict_cache();
    let (ticks, _) = profiling::timed("shade_soa_autovec", || {
        std::hint::black_box(shade_soa(&nm.nx, &nm.ny, &nm.nz, sun));
    });
    println!(
        "  auto-vec scalar (1 acc):  {:>5.1} GB/s",
        count_gb_per_sec(ticks, Some(bytes))
    );

    #[cfg(target_arch = "aarch64")]
    {
        evict_cache();
        let (ticks, _) = profiling::timed("shade_vector_1acc", || {
            std::hint::black_box(unsafe { shade_vector_1acc(&nm.nx, &nm.ny, &nm.nz, sun) });
        });
        println!(
            "  NEON explicit 1 acc:      {:>5.1} GB/s  (4 f32/iter, serial dep)",
            count_gb_per_sec(ticks, Some(bytes))
        );

        evict_cache();
        let (ticks, _) = profiling::timed("shade_vector_4acc", || {
            std::hint::black_box(unsafe { shade_vector_4acc(&nm.nx, &nm.ny, &nm.nz, sun) });
        });
        println!(
            "  NEON explicit 4 acc:      {:>5.1} GB/s  (16 f32/iter, 4 parallel chains)",
            count_gb_per_sec(ticks, Some(bytes))
        );

        evict_cache();
        let (ticks, _) = profiling::timed("shade_vector_8acc", || {
            std::hint::black_box(unsafe { shade_vector_8acc(&nm.nx, &nm.ny, &nm.nz, sun) });
        });
        println!(
            "  NEON explicit 8 acc:      {:>5.1} GB/s  (32 f32/iter, 8 parallel chains)",
            count_gb_per_sec(ticks, Some(bytes))
        );
    }

    println!();
    println!("  single-thread memory ceiling: ~68 GB/s (Exp 1 row-major baseline)");
    println!("  if 4acc >> 1acc: original was compute-bound, not memory-bound");
    println!("  if 4acc ≈ 8acc:  memory-bound — adding more accumulators doesn't help");
}

// ---------------------------------------------------------------------------
// Experiment 4: AoS vs SoA — normal map layout
// ---------------------------------------------------------------------------
//
// SoA (current NormalMap):  nx[0..N], ny[0..N], nz[0..N] — 3 separate Vec<f32>
// AoS (alternative):        [[nx, ny, nz]; N] — 12 bytes per pixel, interleaved
//
// Kernel A — full dot product:  sum += nx*sx + ny*sy + nz*sz  (all 3 used)
//   Bytes/pixel: 12  (identical for both layouts)
//   Hypothesis: comparable — same total traffic, similar prefetch behaviour
//
// Kernel B — nz-only sum:  sum += nz[i]  (1 of 3 used)
//   SoA bytes/pixel: 4  (only the nz array touched)
//   AoS bytes/pixel: 12 (cache must load whole [nx,ny,nz] to reach nz)
//   Hypothesis: SoA wins ~3× because it wastes 0 cache bandwidth

#[inline(never)]
fn shade_soa(nx: &[f32], ny: &[f32], nz: &[f32], sun: [f32; 3]) -> f32 {
    let mut sum = 0.0f32;
    for i in 0..nx.len() {
        sum += nx[i] * sun[0] + ny[i] * sun[1] + nz[i] * sun[2];
    }
    sum
}

#[inline(never)]
fn shade_aos(aos: &[[f32; 3]], sun: [f32; 3]) -> f32 {
    let mut sum = 0.0f32;
    for n in aos {
        sum += n[0] * sun[0] + n[1] * sun[1] + n[2] * sun[2];
    }
    sum
}

#[inline(never)]
fn sum_nz_soa(nz: &[f32]) -> f32 {
    let mut sum = 0.0f32;
    for &v in nz {
        sum += v;
    }
    sum
}

#[inline(never)]
fn sum_nz_aos(aos: &[[f32; 3]]) -> f32 {
    let mut sum = 0.0f32;
    for n in aos {
        sum += n[2];
    }
    sum
}

pub(crate) fn bench_aos_vs_soa(nm: &terrain::NormalMap) {
    let n = nm.rows * nm.cols;
    let sun = [0.4f32, 0.5f32, 0.7f32];
    let bytes_full = n * 12; // 3 × f32 per pixel
    let bytes_nz = n * 4; // 1 × f32 per pixel

    // Build AoS layout once (excluded from benchmark timing)
    let aos: Vec<[f32; 3]> = (0..n).map(|i| [nm.nx[i], nm.ny[i], nm.nz[i]]).collect();

    println!("--- Phase 6 / Exp 4: AoS vs SoA Normal Storage ---");
    println!(
        "  {} Mpix  |  SoA: 3×Vec<f32> ({} MB each)  |  AoS: Vec<[f32;3]> ({} MB total)",
        n / 1_000_000,
        bytes_nz / 1_000_000,
        bytes_full / 1_000_000
    );
    println!();

    // --- Kernel A: full dot product (all 3 components) ---
    evict_cache();
    let (ticks, _) = profiling::timed("soa_dot", || {
        std::hint::black_box(shade_soa(&nm.nx, &nm.ny, &nm.nz, sun));
    });
    let soa_dot = count_gb_per_sec(ticks, Some(bytes_full));

    evict_cache();
    let (ticks, _) = profiling::timed("aos_dot", || {
        std::hint::black_box(shade_aos(&aos, sun));
    });
    let aos_dot = count_gb_per_sec(ticks, Some(bytes_full));

    println!("  dot(normal, sun)  — 12 bytes/pixel");
    println!("    SoA: {:>6.1} GB/s", soa_dot);
    println!(
        "    AoS: {:>6.1} GB/s  ({:.2}× vs SoA)",
        aos_dot,
        aos_dot / soa_dot
    );
    println!();

    // --- Kernel B: nz-only (1 of 3 components) ---
    evict_cache();
    let (ticks, _) = profiling::timed("soa_nz", || {
        std::hint::black_box(sum_nz_soa(&nm.nz));
    });
    let soa_nz = count_gb_per_sec(ticks, Some(bytes_nz));

    evict_cache();
    let (ticks, _) = profiling::timed("aos_nz", || {
        std::hint::black_box(sum_nz_aos(&aos));
    });
    // Report at 4 bytes (logical) — shows effective useful throughput
    let aos_nz = count_gb_per_sec(ticks, Some(bytes_nz));
    // Also show at 12 bytes (actual cache traffic) — shows raw bandwidth used
    let aos_nz_actual = count_gb_per_sec(ticks, Some(bytes_full));

    println!("  nz-only  — 4 bytes/pixel SoA, 12 bytes/pixel AoS (logical 4 reported)");
    println!(
        "    SoA: {:>6.1} GB/s  ({} MB read)",
        soa_nz,
        bytes_nz / 1_000_000
    );
    println!(
        "    AoS: {:>6.1} GB/s  (actual cache traffic: {:.1} GB/s for {} MB)",
        aos_nz,
        aos_nz_actual,
        bytes_full / 1_000_000
    );
    println!("    SoA advantage: {:.1}×", soa_nz / aos_nz);
    println!();
    println!("  L1D = 128 KB (M4 Max per perf core)");
    println!("  AoS nz-only wastes 2/3 of every cache line — nx and ny loaded but discarded");
    println!();

    // --- Kernel B parallel: break the serial dependency chain with rayon ---
    // Each thread has its own accumulator → no loop-carried dependency
    // Now the bottleneck shifts from compute to memory bandwidth → SoA advantage visible
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(10)
        .build()
        .unwrap();

    evict_cache();
    let (ticks, _) = profiling::timed("soa_nz_par", || {
        let sum = pool.install(|| nm.nz.par_iter().map(|&v| v as f64).sum::<f64>());
        std::hint::black_box(sum);
    });
    let soa_nz_par = count_gb_per_sec(ticks, Some(bytes_nz));

    evict_cache();
    let (ticks, _) = profiling::timed("aos_nz_par", || {
        let sum = pool.install(|| aos.par_iter().map(|n| n[2] as f64).sum::<f64>());
        std::hint::black_box(sum);
    });
    let aos_nz_par = count_gb_per_sec(ticks, Some(bytes_nz));
    let aos_nz_par_actual = count_gb_per_sec(ticks, Some(bytes_full));

    println!("  nz-only parallel (10 threads) — dependency chain broken");
    println!(
        "    SoA: {:>6.1} GB/s  ({} MB read)",
        soa_nz_par,
        bytes_nz / 1_000_000
    );
    println!(
        "    AoS: {:>6.1} GB/s  (actual: {:.1} GB/s, {} MB read)",
        aos_nz_par,
        aos_nz_par_actual,
        bytes_full / 1_000_000
    );
    println!("    SoA advantage: {:.1}×", soa_nz_par / aos_nz_par);
}

pub(crate) fn bench_tile_size_sweep(hm: &Heightmap) {
    let rows = hm.rows;
    let cols = hm.cols;
    let pixels = (rows - 2) * (cols - 2);
    let bytes_per_run = pixels * 14; // 5 × i16 reads + 1 × f32 write

    println!("--- Phase 6 / Exp 1: Tile Size Sweep ---");
    println!(
        "  kernel: 5-point stencil sum, cold cache, {} Mpix",
        pixels / 1_000_000
    );
    println!("  logical bytes/run: {:.0} MB", bytes_per_run as f64 / 1e6);
    println!();

    // Baseline: row-major
    let mut output = vec![0.0f32; rows * cols];
    evict_cache();
    let (ticks, _) = profiling::timed("stencil_rowmajor", || {
        stencil_rowmajor(hm, &mut output);
        std::hint::black_box(&output);
    });
    println!(
        "  row-major (baseline):  {:.2} GB/s",
        count_gb_per_sec(ticks, Some(bytes_per_run))
    );

    // Tiled variants — build each layout then evict before timing
    for &tile_size in &[32usize, 64, 128, 256] {
        let tiled = TiledHeightmap::from_heightmap(hm, tile_size);
        let mut output = vec![0.0f32; rows * cols];
        evict_cache();
        let (ticks, _) = profiling::timed("stencil_tiled", || {
            stencil_tiled(&tiled, &mut output);
            std::hint::black_box(&output);
        });
        println!(
            "  tiled  T={:>3} ({:>4} KB/tile):  {:.2} GB/s",
            tile_size,
            tile_size * tile_size * 2 / 1024, // KB: i16 per tile
            count_gb_per_sec(ticks, Some(bytes_per_run))
        );
    }

    println!();
    println!("  L1D = 128 KB (M4 Max per perf core)");
    println!("  T= 32 tile:   2 KB input  — trivially fits L1D");
    println!("  T= 64 tile:   8 KB input  — fits L1D with room for output");
    println!("  T=128 tile:  32 KB input  — fits L1D, output pressure starts");
    println!("  T=256 tile: 128 KB input  — fills entire L1D, spills to L2");
}
