# fill_nodata Algorithm Improvement

## Current implementation

`get_value_from_neighbours` searches outward from each nodata cell independently in 4 directions, stopping at the first valid value per direction.

**Complexity:**
- Per nodata cell: O(gap_size) — walks up to max(rows, cols) steps per direction
- Total: O(N² × avg_gap_size), worst case **O(N³)** where N = max(rows, cols)
- Memory: O(1) extra (plus a tiny Vec<i16> per nodata call — see heap allocation note)

**Additional issue:** `get_value_from_neighbours` allocates a `Vec<i16>` on every call (~654K heap allocations for this tile). Should be replaced with 4 local `Option<i16>` variables — zero allocation, same logic.

---

## Proposed improvement: 4-pass O(N²) sweep

Instead of each nodata cell searching outward, run 4 linear sweeps that precompute the nearest valid value in each direction for every cell simultaneously.

**Left→right sweep (per row):**
```
last_seen = None
for each cell left to right:
    if data[cell] != nodata → last_seen = Some(data[cell])
    if data[cell] == nodata → left_fill[cell] = last_seen
```

**Right→left sweep (per row):** → `right_fill`
**Top→bottom sweep (per column):** → `up_fill`
**Bottom→top sweep (per column):** → `down_fill`

For each nodata cell, average the `Some(v)` values across all 4 directional fills.

**Complexity:**
- 4 sweeps × O(N²) = **O(N²)** total
- Memory: 4 auxiliary arrays × rows × cols × 2 bytes = ~104 MB for a 3601×3601 tile

**Cache behaviour:** each sweep is sequential row-major or column-major — hardware prefetcher trains well. The current approach does strided column walks (stride = cols × 2 bytes = 7202 bytes) on every nodata cell, causing L1/L2 misses on every vertical step.

---

## Tradeoffs

| | Current | 4-pass |
|---|---|---|
| Time complexity | O(N² × gap), worst O(N³) | O(N²) |
| Extra memory | O(1) | O(N²) ~104 MB |
| Cache behaviour | poor (strided column walks) | good (sequential sweeps) |
| Code complexity | low | moderate |

## When to implement

Phase 6 experiment matrix. `fill_nodata` runs once at load time, not in the render hot loop. For SRTM data with narrow gaps the current approach is fast enough in practice. The 4-pass approach becomes relevant if:
- Larger tiles are used (Phase 7 out-of-core)
- Tiles with large contiguous nodata regions are processed
- Load time becomes a bottleneck worth profiling
