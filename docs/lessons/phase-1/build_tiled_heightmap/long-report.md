# Long Report: Phase 1 — Build Tiled Heightmap

---

## Part 1: Why Tiling? — The Cache Locality Problem

### 1.1 What the CPU cache hierarchy does

Every memory access first checks the L1 data cache, then L2, then L3, and only then goes to DRAM. The penalty grows:

| Level | Latency (cycles) | Size (M4 Max) |
|---|---|---|
| L1D | ~4 | 128 KB per perf core |
| L2 | ~12 | 16 MB (shared) |
| L3 | ~40 | 48 MB (shared) |
| DRAM | ~100–300 | — |

The cache does not load one byte at a time. It loads a **cache line** — 64 bytes at a time on all modern x86 and ARM chips. The hardware prefetcher also detects stride-1 sequential patterns and speculatively loads ahead.

### 1.2 The row-major neighbour problem

`Heightmap.data` is stored in row-major order: element `(r, c)` is at index `r * cols + c`. For our tile N47E011, `cols = 3601`.

Phase 2 (normals) will compute for each cell `(r, c)`:
```
north = data[(r-1)*cols + c]
south = data[(r+1)*cols + c]
west  = data[r*cols + (c-1)]
east  = data[r*cols + (c+1)]
```

West and east are at offsets ±1 — almost always in the same or adjacent cache line. North and south are at offsets ±3601 elements = ±7202 bytes = **±113 cache lines away**. There is no hardware prefetcher pattern that covers this. Every north/south read is a cache miss when the working set exceeds L1.

### 1.3 What tiling achieves

In a tiled layout, all cells within one tile are stored contiguously. For tile_size=128:
- A 128×128 tile contains 16,384 cells × 2 bytes = **32,768 bytes = 32 KB**.
- The full 128×128 tile fits in M4's 128 KB L1D.
- When you load cell `(r, c)`, its north `(r-1, c)` is at offset `-tile_size` = -128 elements = -256 bytes = -4 cache lines — almost certainly still in L1.

Cache line utilisation goes from ~1 useful element per 32 loaded (row-major patch read) to 32/32 (tiled patch read).

---

## Part 2: Tile Layout Mathematics

### 2.1 Grid to tile grid

```
tile_rows = (rows + tile_size - 1) / tile_size   // ceiling division
tile_cols = (cols + tile_size - 1) / tile_size

total_elements = tile_rows * tile_cols * tile_size * tile_size
```

**Integer ceiling division.** `(n + d - 1) / d` is the standard idiom for `ceil(n / d)` using integer arithmetic. Works because adding `d-1` forces a round-up when `n` is not a multiple of `d`.

For 3601×3601, tile_size=128: `tile_rows = tile_cols = 29`, `total_elements = 29×29×128×128 = 13,893,632` vs `rows×cols = 12,967,201`. ~7% padding overhead for partial border tiles.

### 2.2 Memory layout diagram

For a 4×4 grid with tile_size=2:

```
[tile(0,0)            ] [tile(0,1)            ] [tile(1,0)            ] [tile(1,1)            ]
 (0,0)(0,1)(1,0)(1,1)   (0,2)(0,3)(1,2)(1,3)   (2,0)(2,1)(3,0)(3,1)   (2,2)(2,3)(3,2)(3,3)
```

Row-major stores: `(0,0)(0,1)(0,2)(0,3)(1,0)...` — 2×2 patch at (0,0)–(1,1) has indices 0,1,4,5 (gap at every row crossing).

Tiled: same patch has indices 0,1,2,3 — all contiguous.

---

## Part 3: Construction — `from_heightmap`

### 3.1 Signature: borrow vs move

```rust
pub fn from_heightmap(hm: &Heightmap, tile_size: usize) -> TiledHeightmap
```

`&Heightmap` borrows: caller retains ownership, can still use `hm` for verification. `TiledHeightmap` owns its own allocation and copies data out — no lifetime coupling.

### 3.2 The four-nested-loop construction

```rust
for tr in 0..tile_rows {
    for tc in 0..tile_cols {
        for r in 0..tile_size {
            for c in 0..tile_size {
                let src_r = (tr * tile_size + r).min(hm.rows - 1);
                let src_c = (tc * tile_size + c).min(hm.cols - 1);
                let src   = src_r * hm.cols + src_c;

                let tile_idx = tr * tile_cols + tc;
                let dst = tile_idx * tile_size * tile_size + r * tile_size + c;

                tiles[dst] = hm.data[src];
            }
        }
    }
}
```

**Clamping** (`.min(hm.rows - 1)`): partial border tiles repeat the last valid row/col. Zero-padding would create artificial elevation cliffs at tile edges, producing wrong normals in Phase 2.

---

## Part 4: Access — `get()`

```rust
#[inline(always)]
pub fn get(&self, row: usize, col: usize) -> i16 {
    let tile_r  = row / self.tile_size;
    let tile_c  = col / self.tile_size;
    let local_r = row % self.tile_size;
    let local_c = col % self.tile_size;

    let tile_idx = tile_r * self.tile_cols + tile_c;
    let dst = tile_idx * self.tile_size * self.tile_size
            + local_r * self.tile_size + local_c;

    self.tiles[dst]
}
```

The `dst` formula is structurally identical to the `dst` formula in `from_heightmap` — divergence = silent wrong values.

**`#[inline(always)]`**: without it, each call is a real function jump. With it, the compiler copies the body into the call site and can hoist tile_r/tile_c out of loops where they're constant. For a hot loop, this is critical.

---

## Part 5: Verification

```rust
assert_eq!(tiled.get(r, c), hm.data[r * hm.cols + c]);
// critical: test tile boundaries
assert_eq!(tiled.get(127, 200), hm.data[127 * hm.cols + 200]);  // last row of tile-row 0
assert_eq!(tiled.get(128, 200), hm.data[128 * hm.cols + 200]);  // first row of tile-row 1
assert_eq!(tiled.get(129, 200), hm.data[129 * hm.cols + 200]);
```

Off-by-one bugs in tiling code produce valid but wrong values — no panic, no crash. Explicit boundary assertions are the only reliable catch.

---

## Part 6: Benchmarking — What the Numbers Revealed

### 6.1 Warm vs cold cache

First run of `bench_neighbours_rowmajor` showed **71.8 GB/s** — almost double the Phase 0 SIMD sequential bandwidth of 37 GB/s. Impossible for a scalar i16 loop.

**Reason**: the heightmap was just parsed 1.5ms earlier. The full grid (~26MB) fits in M4's 48MB L3 cache. The benchmark was measuring L3 bandwidth (~100–200 GB/s), not DRAM bandwidth.

**Fix**: allocate a 64MB scratch buffer and touch every element just before the benchmark. This evicts the 26MB heightmap from L3. `black_box` prevents the compiler from eliminating the allocation.

```rust
let evict: Vec<i32> = (0..16 * 1024 * 1024).map(|i| i as i32).collect();
std::hint::black_box(&evict);
```

After eviction: **26.0 GB/s**. The lesson: always verify your benchmark is running on cold data when measuring memory-bound workloads.

### 6.2 Why row-major gets 26 GB/s (not 5 GB/s)

Phase 0 scalar sequential: 5.2 GB/s. Row-major neighbour benchmark: 26.0 GB/s. Why the difference?

- West/east reads are stride-1 — the hardware prefetcher handles these perfectly.
- North/south reads are stride-3601 — but this is a **constant stride**. The M4 prefetcher detects constant strides and prefetches ahead. 26 GB/s is a blend of stride-1 (fast) and stride-3601 (partially prefetched).

### 6.3 Integer overflow in sum

Summing i16 values into `i32`:
- 3599 × 3599 × 4 reads × ~1500m average ≈ 77 billion >> max i32 (~2.1 billion).
- In release mode, Rust wraps on overflow silently — no panic, but wrong result.
- Use `i64` (max ~9.2 × 10¹⁸) for accumulating elevation sums.
- To check: run in debug mode (`cargo run` without `--release`) — Rust panics on overflow in debug.

### 6.4 Tiled with row-major iteration: 3.7–4.0 GB/s

Using `get()` with the same `r` outer, `c` inner loop structure:
- As `c` increases from 0 to 3600, tile_c changes every 128 columns — 29 × 32KB jumps per row.
- The prefetcher cannot predict these jumps.
- `#[inline(always)]` improved from 3.7 → 4.0 GB/s (call overhead eliminated, but memory pattern unchanged).

**Iteration order must match storage order.** Tiled storage with row-major iteration is worse than row-major storage with row-major iteration.

### 6.5 Tiled with tile-order iteration: 3.0 GB/s

```rust
for tr in 0..hm.tile_rows {
    for tc in 0..hm.tile_cols {
        for r in 0..hm.tile_size {
            for c in 0..hm.tile_size {
                let global_row = tr * hm.tile_size + r;
                let global_col = tc * hm.tile_size + c;
                if global_row == 0 || global_row >= hm.rows - 1
                || global_col == 0 || global_col >= hm.cols - 1 {
                    continue;
                }
                sum += hm.get(global_row - 1, global_col) as i64;
                // ...
            }
        }
    }
}
```

Still 3.0 GB/s — even slower. Root cause: **`get()` recomputes the full tile decomposition on every call**. For each of the 4 neighbours:
```
tile_r  = global_row / tile_size    // division
local_r = global_row % tile_size    // modulo
tile_c  = global_col / tile_size
local_c = global_col % tile_size
tile_idx = tile_r * tile_cols + tile_c
dst = tile_idx * tile_size² + local_r * tile_size + local_c
```

4 neighbours × 6 operations = 24 index computations per cell. Even with inlining and LLVM hoisting some of these, the overhead dominates.

**The `get()` abstraction cannot demonstrate the tiling benefit.** Phase 2 normals will compute tile_idx once per tile, get a raw slice pointer, and use direct `ptr[local_r * tile_size + local_c]` arithmetic with ±1 offsets. Only tile-boundary cells need cross-tile accesses.

### 6.6 Bug: tile-order benchmark with wrong loop bounds

First attempt used `1..tile_size-1` for the inner loops, intending to skip border cells. This was wrong in two ways:

1. **Skips valid interior cells**: local_r=0 in tile (1,0) is global_row=128 — a valid interior cell, not a border.
2. **Doesn't protect against padding**: for tr=28 (last tile row), local_r=17..127 correspond to global_row > 3600 — out of bounds for `get()`.

Fix: iterate full `0..tile_size` and guard with a single `if` checking global bounds.

---

## Part 7: Tile Size Selection

Rule: `tile_size² × 2 bytes × concurrent_tiles ≤ L1D size`

| tile_size | Tile bytes | Tiles in M4 L1 (128 KB) | Pages (4KB) per tile |
|---|---|---|---|
| 64 | 8 KB | 16 | 2 |
| 128 | 32 KB | 4 | 8 |
| 256 | 128 KB | 1 | 32 |

128 is the default. 4 concurrent tiles fit — enough for a 3-point stencil. TLB: 8 pages × 4 tiles = 32 entries out of 192 available on M4.

---

## Part 8: AlignedBuffer — Aligned Memory Allocation

### 8.1 Why Vec<i16> is not enough

`vec![0i16; n]` allocates with 2-byte alignment (natural alignment of `i16`). For SIMD loads in Phase 2, and for TLB/prefetcher efficiency, we want the buffer to start on a 4096-byte page boundary.

**Why you can't just wrap aligned memory in `Vec<i16>`**: when `Vec` drops, it calls `dealloc` with `align_of::<i16>() = 2`. If you allocated with alignment 4096, passing alignment 2 to the deallocator is undefined behavior — the allocator may corrupt its internal bookkeeping or crash.

### 8.2 `std::alloc::Layout`

`Layout` describes a memory allocation request: size in bytes and alignment requirement.

```rust
let layout = std::alloc::Layout::from_size_align(
    len * std::mem::size_of::<i16>(),  // total bytes, not elements
    4096                                // alignment
).unwrap();
```

`from_size_align` returns `Result` because invalid alignments (non-power-of-two, or size not a multiple of alignment when required) are rejected.

### 8.3 `alloc_zeroed`

```rust
let ptr = unsafe { std::alloc::alloc_zeroed(layout) } as *mut i16;
```

- Returns `*mut u8` — a raw byte pointer. Cast to `*mut i16` for typed access.
- `alloc_zeroed` initialises all bytes to 0 (equivalent to `calloc`). All elements start as `0i16`.
- `unsafe` because the compiler cannot verify the pointer will be used correctly.

### 8.4 The `AlignedBuffer` struct

```rust
pub(crate) struct AlignedBuffer {
    ptr: *mut i16,
    len: usize,
    layout: std::alloc::Layout,
}
```

Holds the raw pointer, element count, and the original `Layout` so `Drop` can deallocate correctly.

`pub(crate)` — visible within `dem_io` but not to external crates. External code accesses tile data through `TiledHeightmap::tiles() -> &[i16]`.

### 8.5 `Drop` — safety-critical deallocation

```rust
impl Drop for AlignedBuffer {
    fn drop(&mut self) {
        unsafe { std::alloc::dealloc(self.ptr as *mut u8, self.layout) }
    }
}
```

`Drop` is a trait, not a regular method. `drop(&mut self)` is called automatically when the value goes out of scope. `dealloc` takes `*mut u8` — cast from `*mut i16`. The same `layout` used to allocate must be used to deallocate — this is why the struct stores it.

### 8.6 `Deref` and `DerefMut`

```rust
impl std::ops::Deref for AlignedBuffer {
    type Target = [i16];
    fn deref(&self) -> &[i16] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl std::ops::DerefMut for AlignedBuffer {
    fn deref_mut(&mut self) -> &mut [i16] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}
```

`Deref` coerces `&AlignedBuffer` to `&[i16]` automatically. This enables:
- `buf[i]` — index access
- `&buf[a..b]` — slice ranges
- `buf.iter()` — iteration
- Passing `&buf` where `&[i16]` is expected

`DerefMut` does NOT repeat `type Target` — it inherits `Target` from `Deref`. Repeating it is a compile error.

`from_raw_parts(ptr, len)` is `unsafe` — your promise that `ptr` is valid, non-null, properly aligned, and `len` elements long. `new()` guarantees this.

### 8.7 Verifying alignment

```rust
assert_eq!(tiled_heightmap.tiles().as_ptr() as usize % 4096, 0);
```

Casting a pointer to `usize` gives its numeric address. If the address is divisible by 4096, it is page-aligned. This assertion confirms the allocator honored the alignment request.

### 8.8 API design: private field + accessor method

```rust
// in TiledHeightmap
tiles: AlignedBuffer,  // private

pub fn tiles(&self) -> &[i16] {
    &self.tiles   // Deref coerces AlignedBuffer to &[i16]
}
```

Making `tiles` private hides `AlignedBuffer` from external crates entirely. The public interface returns `&[i16]` — a standard type. The backing storage can be changed later (e.g., to mmap, huge pages) without changing the public API.

---

## Part 9: Rust Patterns Used

| Pattern | Example |
|---|---|
| Ceiling division | `(n + d - 1) / d` |
| Clamp | `x.min(limit - 1)` |
| `#[inline(always)]` | Force inlining for hot-path accessors |
| `OnceLock<T>` | Lazy global init, computed once, cached forever |
| `Option<T>` | `Some(x)` = value, `None` = absent; replaces null |
| `*mut T` raw pointer | No lifetime, no borrow check; requires `unsafe` |
| `Layout` | Size + alignment specification for manual alloc |
| `alloc_zeroed` | Aligned zero-initialised allocation |
| `Drop` trait | Custom destructor — called automatically on scope exit |
| `Deref` / `DerefMut` | Coerce custom type to slice — enables `[]` indexing |
| `pub(crate)` | Visible within crate, hidden from external crates |
| Debug mode overflow | `cargo run` (no --release) panics on integer overflow |
