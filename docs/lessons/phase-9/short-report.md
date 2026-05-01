# Phase 9 — Short Report: Multi-Tile Terrain Streaming

## Data Format

| Property | Value |
|---|---|
| GLO-30 tile size | 3600×3600 (pixel-is-area) |
| Pixel spacing | 1/3600° ≈ 30m |
| Adjacent tile gap | exactly 1/3600° — simple concatenation, no deduplication |
| 3×3 assembled size | 10800×10800 px |
| CRS | EPSG:4326 |

Pixel-is-area: centres at ±0.5/3600° from integer degree boundary. No shared edge pixels
(contrast with SRTM 3601×3601 pixel-is-point).

---

## Assembly

```rust
// row-interleaved: for each pixel row, copy across all 3 tile columns
for tile_row in 0..3 {
    for pixel_row in 0..nw_tile.rows {
        for tile_col in 0..3 {
            copy grid[tile_row][tile_col].data[pixel_row * cols .. (pixel_row+1) * cols]
        }
    }
}
```

Metadata (origin, dx_deg, crs_epsg) taken from NW tile — its origin is the assembled grid origin.

---

## GPU Scene Update (`update_heightmap`)

**Three requirements for in-place re-upload:**

1. `_ao_texture: wgpu::Texture` must be stored — view keeps GPU resource alive but `write_texture` needs the Texture object
2. Normal buffers need `COPY_DST` flag — `create_buffer_init` with `STORAGE` only → `write_buffer` fails
3. Mip chain must be regenerated after writing mip 0 — extracted to `write_hm_mips(queue, texture, base, cols, rows)`

Bind group does **not** need to change — `write_texture`/`write_buffer` update GPU memory in-place.

---

## Coordinate Inversion (EPSG:4326)

Forward (used at startup):
```
x = (lon - origin_lon) / dx_deg * dx_meters
y = (origin_lat - lat) / dy_deg.abs() * dy_meters
```

Inverse (used for crossing detection and cam_pos re-projection):
```
pixel_col = cam_x / dx_meters
pixel_row = cam_y / dy_meters
lon = origin_lon + pixel_col * dx_deg.abs()
lat = origin_lat - pixel_row * dy_deg.abs()
```

Crossing: `floor(lon) != centre_lon || floor(lat) != centre_lat`

---

## Sliding Window Thread Pattern

```
main thread                       tile loader thread
    |                                    |
    |── (new_lat, new_lon) ──────────>   | load_grid + normals + AO
    |                                    |
    |<── TileBundle ─────────────────    |
    |
    | update_heightmap (GPU upload, main thread only)
    | re-project cam_pos via latlon round-trip
    | respawn shadow worker with new Arc<Heightmap>
    | update centre_lat / centre_lon
```

`SyncSender` capacity 1: stale crossing requests dropped automatically.
Old shadow worker exits when its sender is replaced (RecvError → loop exits).

---

## Bugs Fixed

### dt = 0 (camera frozen)

`get_current_texture()` previously acted as an accidental vsync gate between
`last_frame = Instant::now()` and `dt = last_frame.elapsed()`. After restructuring, this gate
was gone. Fix: measure `dt` before resetting `last_frame`:

```rust
let dt = self.last_frame.elapsed().as_secs_f32();  // ← time since prev frame
self.last_frame = std::time::Instant::now();
```

### Shadow recomputing every frame (466 MB upload every 0.5s)

`shadow_computing` flag prevents concurrent work but not redundant recomputes. Fix: track last
dispatched azimuth/elevation; gate on ≥ 0.1° sun movement:

```rust
let sun_moved = (azimuth - self.last_shadow_az).abs() > 0.00175
    || (elevation - self.last_shadow_el).abs() > 0.00175;
if !self.shadow_computing && elevation > 0.0 && sun_moved { ... }
```

---

## Startup Timing (Intel Mac, 10800×10800)

| Stage | Time | Note |
|---|---|---|
| load_grid (9 × DEFLATE COG) | 4.52s | Disk I/O |
| normals (parallel) | 185ms | |
| shadows (parallel) | 525ms | |
| AO (16-azimuth DDA) | 7.81s | Bottleneck — 116M px × 16 sweeps |

AO crop optimisation (Step 3): limit to ~10km radius = ~667×667 px → ~290× speedup → ~27ms.

---

## Open Items

| Item | Step |
|---|---|
| Windowed GeoTIFF extraction (`extract_window`) | Step 2 |
| Per-tier background loader threads + AO crop | Step 3 |
| Multi-source-tile stitching | Step 4 |
| Multi-tier shader (30m/5m/1m lerp blend) | Step 5 |
| Download 16 outer tiles (5×5 grid) | — |
| Shadow mask as u8 not f32 (116 MB vs 466 MB) | — |
