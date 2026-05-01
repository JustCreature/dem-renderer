# Phase 9 — Long Report: Multi-Tile Terrain Streaming

## Overview

Phase 9 implements Step 1 of the multi-tile multi-resolution terrain plan: a 3×3 sliding window
of Copernicus GLO-30 30m tiles that tracks the camera across tile boundaries without interrupting
rendering. Along the way it exposed several bugs and architectural gaps — all measured and fixed.

---

## 1. The Data Format: Copernicus GLO-30

### Pixel convention: pixel-is-area vs pixel-is-point

SRTM tiles are 3601×3601 — the extra row/column is a duplicate of the adjacent tile's edge
(pixel-is-point convention: sample at the corner of each cell). Copernicus GLO-30 tiles are
3600×3600 (pixel-is-area: sample at the centre of each cell).

This matters for assembly. If GLO-30 were pixel-is-point, you would need to deduplicate the
shared edge: assembled width = 3 × 3601 − 2 = 10801. Instead, for pixel-is-area, there are no
shared pixels. Adjacent tiles simply abut.

Verification via `gdalinfo`: the pixel spacing is 1/3600° and the centres of the last pixel of
tile N and the first pixel of tile N+1 are separated by exactly 1/3600° — one pixel spacing,
as if they were adjacent pixels in a single large raster. Simple concatenation, zero deduplication.

Assembled 3×3 grid: 3 × 3600 = **10800 × 10800 pixels**.

### Coordinate system

Each tile's origin (top-left corner of the NW pixel) is at the integer degree boundary + 0.5/3600°
(half a pixel south and east of the corner). For assembly, `assemble_grid` takes the NW tile's
`origin_lat` and `origin_lon` as the origin of the assembled heightmap — these fields already
encode the correct position.

---

## 2. Grid Assembly Architecture

### Row-interleaved assembly

Naïve approach: copy all rows of tile (0,0), then all rows of tile (0,1), then tile (0,2).
Wrong — this produces a column-tiled layout, not a row-major image.

Correct approach: for each pixel row, iterate across all three tile columns:

```
for tile_row in 0..3 {
    for pixel_row in 0..nw_tile.rows {
        for tile_col in 0..3 {
            copy hm[tile_row][tile_col].data[pixel_row * cols .. (pixel_row+1) * cols]
        }
    }
}
```

This interleaves pixels correctly: row 0 of the assembled image = row 0 of tile (0,0), row 0 of
tile (0,1), row 0 of tile (0,2); row 1 = row 1 of (0,0), row 1 of (0,1), row 1 of (0,2); etc.

### Injectable loader

`load_grid<F>(tiles_dir, centre_lat, centre_lon, loader: F)` takes a closure
`F: Fn(&Path) -> Option<Heightmap>`. This decouples the grid logic from the I/O mechanism —
the same function can be used with `parse_geotiff`, a mock, or a cached reader.

The closure returns `Option` because tiles may be missing (ocean, edge of dataset). Missing tiles
produce zero-height patches in the assembled grid.

### Metadata from NW tile

The assembled `Heightmap` takes `origin_lat`, `origin_lon`, `dx_deg`, `dy_deg`, `dx_meters`,
`dy_meters`, `crs_epsg` from the NW tile. This is correct because:
- The NW tile's origin is the top-left corner of the assembled grid
- All tiles share the same pixel spacing (GLO-30 is uniform 1/3600°)
- All tiles are EPSG:4326

---

## 3. GPU Upload: What `update_heightmap` Needed

### The `_ao_texture` gap

`GpuScene::new()` creates an `ao_texture: wgpu::Texture`, writes to it, creates a view and
sampler — then drops the `Texture` binding at end of scope. Only `_ao_view` and `_ao_sampler`
were stored. The view keeps the GPU resource alive via reference counting, so rendering worked.

But `write_texture` requires the `Texture` object, not the view. To re-upload AO on a tile
slide, the texture must be stored: `_ao_texture: wgpu::Texture` added to `GpuScene`.

### Normal buffers need `COPY_DST`

Normal buffers were created with `create_buffer_init` and `usage: STORAGE` only. `write_buffer`
requires `COPY_DST`. Adding this flag to all three (`nx`, `ny`, `nz`) enables in-place update
without recreating the bind group.

**Why the bind group doesn't need to change:** `write_texture` and `write_buffer` update GPU
memory in-place. The bind group stores a reference to the GPU resource object, not a snapshot
of its contents. After a `write_buffer`, the next `dispatch_workgroups` sees the new data
automatically — the bind group entry is unchanged.

### Mip chain must be regenerated

The heightmap texture has 8 mip levels (for LOD in the shader). `write_texture` on mip level 0
does not propagate to levels 1–7. The max-filter mip generation loop must run again after every
`update_heightmap`. Extracted to a free function `write_hm_mips(queue, texture, base, cols, rows)`
called from both `new()` and `update_heightmap()`.

---

## 4. The Sliding Window

### Coordinate inversion

The viewer stores camera position as `cam_pos: [f32; 3]` in tile-local metres from the
assembled grid's NW corner. To detect tile crossing, this needs to be inverted to WGS84 lon/lat.

For EPSG:4326, the forward projection is linear:
```
x = (lon - origin_lon) / dx_deg * dx_meters
y = (origin_lat - lat) / dy_deg.abs() * dy_meters
```

Inversion:
```
pixel_col = cam_x / dx_meters
pixel_row = cam_y / dy_meters
lon = origin_lon + pixel_col * dx_deg.abs()
lat = origin_lat - pixel_row * dy_deg.abs()
```

`floor(lon)` and `floor(lat)` give the integer-degree tile origin. Compare against `centre_lon`
and `centre_lat` each frame to detect a crossing.

### Background thread pattern

The tile loader mirrors the existing shadow worker pattern:

```
tile_tx: SyncSender<(i32, i32)>   ← main thread sends new centre
tile_rx: Receiver<TileBundle>      ← main thread receives completed bundle
```

`SyncSender` with capacity 1: if the camera crosses another boundary before the first load
finishes, the new request replaces the pending one. No queue buildup.

The loader thread computes: `load_grid` → `compute_normals_vector_par` → `compute_ao_true_hemi`
→ sends `TileBundle { hm, normals, ao }`. All CPU-heavy work is off the frame thread.

GPU upload (`update_heightmap`) happens on the main thread when the bundle arrives, because wgpu
requires all `write_texture`/`write_buffer` calls on the device thread.

### Camera re-projection on slide

When a new grid arrives, `cam_pos` is in the old grid's coordinate system. Before swapping the
heightmap reference, convert the current position back to WGS84, then forward-project into the
new grid:

```rust
let (lat, lon) = tile_meters_to_latlon_epsg_4326(cam_pos[0], cam_pos[1], &old_hm);
if let Some((nx, ny)) = latlon_to_tile_metres(lat, lon, &new_hm) {
    cam_pos[0] = nx;
    cam_pos[1] = ny;
}
```

This is exact for EPSG:4326 (linear projection). The camera appears to stay still while the
terrain grid silently shifts under it.

### Shadow worker lifecycle

The shadow worker holds an `Arc<Heightmap>`. On tile slide, a new worker is spawned with the
new `Arc`. The old worker is killed implicitly:

- Old channel: `(shadow_tx, worker_rx)` — worker holds `worker_rx`
- When `self.shadow_tx` is replaced, the old sender drops
- The old worker's `worker_rx.recv()` returns `Err(RecvError)` → `while let Ok(...)` exits
- Thread terminates within one shadow computation cycle (≤525ms)

No explicit join or abort needed.

---

## 5. Bugs Found and Fixed

### Bug 1: `dt` was near-zero

**Symptom:** camera appeared frozen — WASD/Space keys had no effect.

**Root cause:** the original code computed `dt = self.last_frame.elapsed()` after
`surface.get_current_texture()`, which blocks waiting for the GPU to be ready. This GPU sync
inadvertently made `elapsed()` measure real inter-frame time (~2ms at 500fps). When the code
was restructured to put the tile-sliding block (which needs `&mut scene`) before the immutable
`let scene: &GpuScene` borrow, `get_current_texture()` was moved after `dt` computation.
With nothing between `last_frame = Instant::now()` and `dt = last_frame.elapsed()`, dt was
a few microseconds — effectively zero movement per frame.

**Fix:** measure dt first, then reset `last_frame`:
```rust
let dt = self.last_frame.elapsed().as_secs_f32();  // elapsed since prev frame
self.last_frame = std::time::Instant::now();
```

**Lesson:** `dt` must measure time *between* frames (from the previous frame's start to now),
not time *within* a frame. Never reset the timer before measuring the interval.

### Bug 2: Shadow recomputed every frame

**Symptom:** camera movement caused rhythmic stuttering at ~0.5s intervals even with a static sun.

**Root cause:** the shadow dispatch condition was:
```rust
if !self.shadow_computing && elevation > 0.0 { ... }
```
This fires every frame the worker is idle, regardless of whether the sun moved. The shadow worker
takes ~525ms for a 10800×10800 grid, then sends back a 10800×10800 × 4 bytes (f32) = 466 MB mask.
`write_buffer` staging this every 0.5 seconds caused regular GPU pipeline stalls.

**Fix:** track the last dispatched azimuth/elevation; only recompute when sun moves ≥ 0.1°:
```rust
let sun_moved = (azimuth - self.last_shadow_az).abs() > 0.00175   // 0.1° in radians
    || (elevation - self.last_shadow_el).abs() > 0.00175;
if !self.shadow_computing && elevation > 0.0 && sun_moved { ... }
```

**Lesson:** background workers should be triggered by state change, not by idle availability.
The `shadow_computing` flag prevents concurrent work but doesn't prevent redundant work.

---

## 6. Startup Timing (Intel Mac, 10800×10800)

| Stage | Time |
|---|---|
| load_grid (9 × DEFLATE COG, disk) | 4.52s |
| compute_normals_vector_par | 185ms |
| compute_shadow_vector_par | 525ms |
| compute_ao_true_hemi (16 sweeps) | 7.81s |

AO dominates at 7.81s for 116M pixels × 16 DDA sweeps. Plan for Step 3: crop AO to a ~10km
radius (~667×667 px at 30m/px) centred on camera. Expected speedup: 116M → 0.4M pixels = ~290×,
dropping AO to ~27ms per reload.

---

## 7. Open Items

- **Step 2:** windowed GeoTIFF extraction — in-process `-projwin` equivalent for BEV 5m/1m tiles
- **Step 3:** per-tier background loader threads + AO crop optimisation (~290× speedup)
- **Step 4:** multi-source-tile stitching (window crossing BEV source tile boundaries)
- **Step 5:** multi-tier shader (30m/5m/1m with lerp blend zones)
- **5×5 tile download:** 16 outer tiles (lat 45–49, lon 9–13) pending; script updated
- **Shadow mask storage:** currently f32 (466 MB for 10800×10800); u8 would be 116 MB
