# Short Report: Phase 1 — Build Tiled Heightmap

---

## 1. Why Tiling

Row-major: north/south neighbours at ±3601 elements = ±7202 bytes = ±113 cache lines. Cache miss on every row crossing. Cache line = 64 bytes = 32 i16 values — ~1 useful element per loaded line.

Tiled: all cells within one tile contiguous. North neighbour at -128 elements = -256 bytes = -4 cache lines (tile_size=128). L1 hit after tile warm-up. 32/32 elements per loaded line.

Phase 0 sequential/random ratio (11–16×) is the empirical ceiling tiling closes.

---

## 2. Tile Grid Dimensions

```rust
tile_rows = (rows + tile_size - 1) / tile_size   // ceiling division
tile_cols = (cols + tile_size - 1) / tile_size
total_elements = tile_rows * tile_cols * tile_size * tile_size  // ≥ rows*cols
```

For 3601×3601, tile_size=128: 29×29 tiles, ~7% padding overhead.

---

## 3. Construction: `from_heightmap`

`hm: &Heightmap` (borrow) — caller retains ownership. `TiledHeightmap` copies data out, no lifetime coupling.

```rust
for tr in 0..tile_rows { for tc in 0..tile_cols {
    for r in 0..tile_size { for c in 0..tile_size {
        let src_r = (tr * tile_size + r).min(hm.rows - 1);  // clamp partial tiles
        let src_c = (tc * tile_size + c).min(hm.cols - 1);
        let src   = src_r * hm.cols + src_c;
        let tile_idx = tr * tile_cols + tc;
        let dst = tile_idx * tile_size * tile_size + r * tile_size + c;
        tiles[dst] = hm.data[src];
    }}
}}
```

Clamping repeats border values — zero-padding would create fake elevation cliffs.

---

## 4. Access: `get()`

```rust
#[inline(always)]
pub fn get(&self, row: usize, col: usize) -> i16 {
    let tile_r  = row / self.tile_size;
    let tile_c  = col / self.tile_size;
    let local_r = row % self.tile_size;
    let local_c = col % self.tile_size;
    let tile_idx = tile_r * self.tile_cols + tile_c;
    self.tiles[tile_idx * self.tile_size * self.tile_size + local_r * self.tile_size + local_c]
}
```

`dst` formula identical to `from_heightmap` — divergence = silent wrong values.

`#[inline(always)]` — without it, 4 function calls per cell with full overhead. With it, LLVM can hoist tile_r/tile_c when they're loop-invariant.

---

## 5. Verification

```rust
assert_eq!(tiled.get(r, c), hm.data[r * hm.cols + c]);
// test tile boundaries: rows 127, 128, 129 for tile_size=128
```

Off-by-one bugs produce valid but wrong values — no panic. Boundary assertions are the only reliable catch.

---

## 6. Benchmark Results (M4 Max, cold cache)

| Benchmark | GB/s | Notes |
|---|---|---|
| row_major neighbour sum | 26–46 | Prefetcher detects stride-3601 for N/S |
| tiled, row-major iteration | 3.7–4.0 | 32KB jumps every 128 cols |
| tiled, tile-order iteration | 3.0 | `get()` decomposition overhead dominates |

**Warm cache trap**: first run showed 71.8 GB/s because heightmap (~26MB) was hot in L3 (just parsed). Fix: evict with a 64MB scratch buffer before benchmarking.

```rust
let evict: Vec<i32> = (0..16 * 1024 * 1024).map(|i| i as i32).collect();
std::hint::black_box(&evict);
```

**Integer overflow**: summing i16 values → use `i64`. `i32` overflows at ~2.1B; realistic sum is ~77B. Debug mode panics on overflow (`cargo run` without `--release`).

**Key lesson**: tiling only helps when iteration order matches storage order AND inner-loop access avoids per-cell decomposition overhead. `get()` is correct for verification, not for hot loops. Phase 2 will use direct tile pointer arithmetic.

---

## 7. Tile Size Selection

| tile_size | Tile bytes | Tiles in M4 L1 (128 KB) | Pages per tile |
|---|---|---|---|
| 64 | 8 KB | 16 | 2 |
| 128 | 32 KB | 4 | 8 |
| 256 | 128 KB | 1 | 32 |

Default: 128. TLB: 8 pages × 4 tiles = 32 entries (M4 L1 TLB has 192).

---

## 8. AlignedBuffer

**Problem**: `Vec<i16>` deallocates with alignment 2. Using it to hold page-aligned memory = UB when dropped.

**Solution**: custom struct with manual allocation and `Drop`.

```rust
pub(crate) struct AlignedBuffer {
    ptr: *mut i16,
    len: usize,
    layout: std::alloc::Layout,
}

impl AlignedBuffer {
    pub fn new(len: usize, align: usize) -> Self {
        let layout = Layout::from_size_align(len * 2, align).unwrap();
        let ptr = unsafe { alloc_zeroed(layout) } as *mut i16;
        AlignedBuffer { ptr, len, layout }
    }
}

impl Drop for AlignedBuffer {
    fn drop(&mut self) {
        unsafe { dealloc(self.ptr as *mut u8, self.layout) }
    }
}

impl Deref for AlignedBuffer {
    type Target = [i16];
    fn deref(&self) -> &[i16] {
        unsafe { from_raw_parts(self.ptr, self.len) }
    }
}

impl DerefMut for AlignedBuffer {
    // NO type Target here — inherited from Deref
    fn deref_mut(&mut self) -> &mut [i16] {
        unsafe { from_raw_parts_mut(self.ptr, self.len) }
    }
}
```

| Item | Detail |
|---|---|
| `Layout` | size in bytes (not elements) + alignment |
| `alloc_zeroed` | returns `*mut u8`, zero-initialised |
| `Drop` trait | called automatically on scope exit, not manually |
| `Deref` | enables `buf[i]`, `&buf[..]`, `.iter()` |
| `pub(crate)` | hidden from external crates |

**Verify alignment**:
```rust
assert_eq!(tiled_heightmap.tiles().as_ptr() as usize % 4096, 0);
```

**API**: `tiles` field is private; exposed as `pub fn tiles(&self) -> &[i16]`. Hides `AlignedBuffer` from public API — backing storage can change without breaking callers.

---

## 9. Rust Patterns

| Pattern | Usage |
|---|---|
| `OnceLock<T>` | Lazy global init — `FREQ.get_or_init(|| { ... })` |
| `Option<T>` | `Some(x)` / `None` — Rust's null replacement, checked at compile time |
| `#[inline(always)]` | Force inlining; critical for hot-path accessor functions |
| `pub(crate)` | Crate-internal visibility |
| `*mut T` | Raw pointer — no lifetime, requires `unsafe` |
| `Drop` trait | Custom destructor |
| `Deref`/`DerefMut` | Coerce to slice — enables `[]` and slice methods |
| Debug overflow check | `cargo run` (no `--release`) panics on integer overflow |
