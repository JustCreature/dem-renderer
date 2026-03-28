# Session Log: Phase 1 — Build Tiled Heightmap
**Date:** 2026-03-22 / 2026-03-28
**Branch:** phase_1

---

## What was covered

### Motivation for tiled layout
- Row-major layout: north/south neighbour access for a 3601-wide grid = ±7202 bytes = ±113 cache lines. Cache miss on every row crossing.
- Cache line = 64 bytes = 32 i16 values. Row-major patch reads waste ~31/32 loaded bytes.
- Tiling: all cells within a tile stored contiguously. North neighbour at -tile_size elements = -256 bytes = -4 cache lines for tile_size=128. L1 hit after tile warm-up.
- Phase 0 sequential/random ratio (11–16×) is the empirical ceiling that tiling closes.

### TiledHeightmap struct
- Added to new file `crates/dem_io/src/tiled.rs` (split from `lib.rs`).
- Fields mirror `Heightmap` geo fields exactly, plus: `tiles`, `tile_size`, `tile_rows`, `tile_cols`.
- Exported via `pub use tiled::TiledHeightmap` in `lib.rs`.

### Tile grid math
- `tile_rows = (rows + tile_size - 1) / tile_size` — ceiling division idiom.
- `total_elements = tile_rows * tile_cols * tile_size * tile_size` — includes padding for partial border tiles.
- For 3601×3601 with tile_size=128: 29×29 tiles, ~7% padding overhead.

### `from_heightmap` implementation
- Signature: `pub fn from_heightmap(hm: &Heightmap, tile_size: usize) -> TiledHeightmap`.
- Borrow (`&Heightmap`) chosen over move — caller retains ownership for verification assertions.
- 4-nested-loop: outer `(tr, tc)` over tile grid, inner `(r, c)` within tile.
- Source index: `src_r = (tr*tile_size + r).min(hm.rows - 1)` — clamp to avoid out-of-bounds on border tiles.
- Destination index: `tile_idx = tr * tile_cols + tc`, `dst = tile_idx * tile_size² + r * tile_size + c`.

### `get()` implementation
- Decompose: `tile_r = row / tile_size`, `local_r = row % tile_size` (similarly for col).
- Recompose: `tile_idx = tile_r * tile_cols + tile_c`, `dst = tile_idx * tile_size² + local_r * tile_size + local_c`.
- `self.tiles[dst]` as implicit return (no semicolon).
- Added `#[inline(always)]` — without it, 4 separate function calls per cell with full overhead.

### Ownership vs borrowing / Option<T>
- `hm: &Heightmap` (borrow) chosen over `hm: Heightmap` (move) — caller retains ownership.
- `Option<T>` explained: Rust's null replacement. `Some(x)` = value present, `None` = absent. Type system forces handling both — can't use `None` as a number.

### i8 vs i16
- User asked how 3477m can fit in i16 if max is 127. Clarified: max 127 is i8 (1 byte). i16 is 2 bytes, range -32768 to 32767 — covers all Earth elevations including ocean trenches.

### OnceLock — caching counter_frequency
- `counter_frequency()` was sleeping 100ms on every call to measure tick frequency.
- Fixed with `static FREQ: std::sync::OnceLock<f64>` — computed once on first call, cached forever.
- `FREQ.get_or_init(|| { ... })` — closure runs exactly once.

### count_gb_per_sec with optional bytes
- Added `Option<usize>` parameter: `None` uses default `N * size_of::<f32>()`, `Some(n)` uses caller-provided bytes.
- Alternative approach discussed: two functions (`count_gb_per_sec` + `count_gb_per_sec_for`) to avoid `None` at every callsite. User chose `Option`.

### Benchmark: bench_neighbours_rowmajor
- Simulates Phase 2 normal computation: read 4 neighbours (N/S/W/E) for every interior cell.
- Bug found: original had swapped r/c indices (`(c-1)*cols+r` instead of `r*cols+(c-1)`).
- `sum: i64` — original `i32` overflows: 12M cells × 4 reads × ~1500m avg ≈ 77 billion >> 2.1B max i32.
- Overflow check: run `cargo run` (debug mode) — panics on overflow. `checked_add` for release-mode checking.

### Cold cache eviction
- First run showed 71.8 GB/s — impossibly high (double Phase 0 SIMD sequential).
- Reason: heightmap was hot in L3 (just parsed, ~26MB fits in 48MB L3). Measuring L3 bandwidth, not DRAM.
- Fix: allocate 64MB scratch buffer and touch it before benchmark to evict heightmap.
- After eviction: 26.0 GB/s — much more believable.
- 26 GB/s > 5.2 GB/s scalar sequential because: prefetcher detects constant stride-3601 for N/S reads, and W/E are stride-1.

### bench_neighbours_tiled (row-major iteration)
- Same loop structure as row-major, but using `hm.get(r±1, c)` etc.
- Without `#[inline(always)]`: 3.7 GB/s.
- With `#[inline(always)]`: 4.0 GB/s — small improvement, not enough.
- Root cause: row-major iteration over tiled storage = 32KB jump every 128 columns. Prefetcher can't handle it.

### bench_neighbours_tiled_walk_tiles_order (tile-order iteration)
- Outer loops `(tr, tc)` over tile grid, inner loops `(r, c)` within tile.
- Bug 1 (first attempt): `1..tile_size-1` skips valid interior tile-edge cells. Only global border should be skipped.
- Bug 2: padding cells in border tiles have `global_row >= hm.rows` — `get()` would be out of bounds.
- Fix: iterate `0..tile_size`, guard with `if global_row == 0 || global_row >= hm.rows - 1 || ...`.
- Result: 3.0 GB/s — even slower than row-major tiled iteration.
- Root cause: `get()` recomputes full decomposition (tile_r, local_r, tile_c, local_c, tile_idx, dst) for all 4 neighbours every cell. Even inlined, this is too much arithmetic. Can't demonstrate tiling benefit through `get()`.
- Lesson: Phase 2 must use direct tile pointer arithmetic, not `get()`, in the inner loop.

### AlignedBuffer
- Goal: allocate `tiles` with 4096-byte (page) alignment — satisfies cache-line alignment, enables page-aligned SIMD loads in Phase 2.
- `Vec<i16>` default allocator: 2-byte alignment only.
- Can't use `Vec::from_raw_parts` with aligned pointer — `Vec::drop` deallocates with `align_of::<i16>()=2`, not 4096. UB.
- Solution: custom struct `AlignedBuffer` in `crates/dem_io/src/aligned.rs`.
- Fields: `ptr: *mut i16`, `len: usize`, `layout: std::alloc::Layout`.
- `new(len, align)`: `Layout::from_size_align(len * 2, align)`, `alloc_zeroed(layout) as *mut i16`.
- `Drop`: `dealloc(self.ptr as *mut u8, self.layout)` — must cast to `*mut u8`, must be `unsafe {}`.
- `Deref<Target=[i16]>`: `from_raw_parts(self.ptr, self.len)` — enables `buf[i]`, slice coercion.
- `DerefMut`: `from_raw_parts_mut` — enables `buf[i] = x`.
- `DerefMut` does NOT have `type Target` — inherits from `Deref`.
- Visibility: `pub(crate)` — internal implementation detail, not part of public API.
- `tiles` field made private, exposed via `pub fn tiles(&self) -> &[i16]`.
- Alignment verified: `assert_eq!(tiled_heightmap.tiles().as_ptr() as usize % 4096, 0)`.

## Issues found and fixed
- Bug: row/col swapped in `bench_neighbours_rowmajor` index formula.
- Bug: `sum: i32` overflow in neighbour benchmarks — changed to `i64`.
- Bug: `1..tile_size-1` skipping valid cells in tile-order benchmark.
- Bug: `DerefMut` had redundant `type Target` — compile error.
- Typo: `prt` instead of `ptr` in `AlignedBuffer` field.
- `drop` method missing `&mut self` parameter.

## Measured numbers (M4 Max, cold cache after eviction)
- row_major neighbour sum: **26.0–46 GB/s** (varies; prefetcher effect visible)
- tiled row-major iteration: **3.7–4.0 GB/s** (get() overhead + 32KB tile jumps)
- tiled tile-order iteration: **3.0 GB/s** (get() decomposition overhead dominates)

## Open items
- `profiling::timed` in random_read, seq_write, random_write uses wrong label `"seq_read"` — fix.
- `fill_nodata` division-by-zero if all 4 directions hit boundary — Option<i16> fix pending.
- Drop `bil_bytes` early in `parse_bil` to halve peak memory.
- Phase 2: normal computation must use direct tile pointer access, not `get()`, in inner loop.
- `AlignedBuffer` is not `Send`/`Sync` — needs `unsafe impl Send/Sync` for rayon in Phase 2.
