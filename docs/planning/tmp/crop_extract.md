# Step 2 Implementation Plan: crop + extract_window

## Context

Step 2 of the multi-tile multi-resolution plan splits into two independent functions
with different purposes:

| Function | File | Purpose |
|---|---|---|
| `crop` | `crates/dem_io/src/grid.rs` | In-memory slice of assembled 30m Heightmap for AO |
| `extract_window` | `crates/dem_io/src/geotiff.rs` | Selective tile-based disk read for 5m/1m tiers |

---

## Implementation order

1. **`crop` first** — the tile loader thread currently runs AO on 10800×10800 (7.8s Intel Mac).
   This blocks the background thread and makes Step 3 unusable. Fix the bottleneck first.
2. **`extract_window` second** — enables 5m and 1m display tiers. Depends on nothing from `crop`.

---

## Part 1: `crop`

### What it does

Takes an already-decoded `Heightmap` (the assembled 10800×10800 30m grid) and returns
a sub-rectangle as a new `Heightmap` with correct metadata.

### Signature

```rust
pub fn crop(hm: &Heightmap, row_start: usize, col_start: usize, rows: usize, cols: usize) -> Heightmap
```

### Implementation notes

- Copy rows `row_start..row_start+rows`, columns `col_start..col_start+cols` into a new `Vec<f32>`.
- Update `crs_origin_x` and `crs_origin_y`:
  - For EPSG:4326: `new_origin_lon = origin_lon + col_start * dx_deg`,
    `new_origin_lat = origin_lat - row_start * dy_deg.abs()`
  - For projected CRS: `new_origin_x = crs_origin_x + col_start * dx_meters`,
    `new_origin_y = crs_origin_y - row_start * dy_meters`
- `rows`, `cols`, `dx_meters`, `dy_meters`, `crs_epsg` are unchanged.
- No NODATA fill needed — the source is already filled.

### Usage in tile loader (Step 3 AO optimisation)

AO radius ~10 km at 30m/px = ~333 px radius → 667×667 px window.
Camera position in pixel space: `cam_px = cam_x / dx_meters`, `cam_py = cam_y / dy_meters`.

```
row_start = (cam_py - 333).max(0) as usize
col_start = (cam_px - 333).max(0) as usize
rows = 667.min(hm.rows - row_start)
cols = 667.min(hm.cols - col_start)
```

Run `compute_ao_true_hemi` on the cropped Heightmap, embed result into a full-size
`Vec<f32>` filled with `1.0`, with the crop result written at `(row_start, col_start)`.

Expected: 116M → 0.44M pixels → ~290× speedup (~27ms instead of 7.8s).

---

## Part 2: `extract_window`

### What it does

Opens a GeoTIFF, reads only the internal 256×256 tiles that intersect the requested
pixel window, copies the relevant pixel strips into an output buffer, returns a `Heightmap`.

No full-image decode. Uses the `tiff` crate's tile API.

### tiff crate API (confirmed from source, v0.11.3)

- `decoder.seek_to_image(ifd_index)` — jump to any IFD directory (0 = full-res, 1 = first overview)
- `decoder.chunk_dimensions()` → `(tile_w, tile_h)`
- `decoder.tile_count()` → total tiles in current IFD
- `decoder.read_chunk(index)` → `DecodingResult` for one tile (decompressed)
- Tile flat index: `row * tiles_across + col`, where `tiles_across = ceil(image_width / tile_w)`

### File layouts (confirmed via tiffinfo)

| File | Full-res dims | Tile size | Tile grid | CRS |
|---|---|---|---|---|
| GLO-30 30m | 3600×3600 | 1024×1024 | 4×4 = 16 tiles | EPSG:4326 |
| BEV DGM 5m | 120001×70001 | 256×256 | 469×274 | EPSG:31287 |
| LiDAR 1m | 50001×50001 | 256×256 | 196×196 | EPSG:3035 |

### Window size vs tiles read

| Tier | Window | px window | Tile size | Tiles read | % of file |
|---|---|---|---|---|---|
| 5m | 10 km | 2000×2000 | 256 | 8×8 = 64 | ~5% |
| 1m | 5 km | 5000×5000 | 256 | 20×20 = 400 | ~6% |

### Signature

```rust
pub fn extract_window(
    path: &Path,
    centre_crs: (f64, f64),   // (easting, northing) or (lon, lat) in file's native CRS
    radius_m: f64,
    ifd_level: usize,         // 0 = full-res, 1+ = overview
) -> Result<Heightmap, DemError>
```

### Algorithm

1. Open file, `seek_to_image(ifd_level)`.
2. Read `ModelPixelScaleTag` (33550) and `ModelTiepointTag` (33922) for affine transform.
3. Convert `centre_crs` → pixel `(cx, cy)` using the affine transform (already done
   in `parse_geotiff_epsg_31287` / `parse_geotiff_epsg_3035` — reuse the same math).
4. Compute pixel bbox: `px0 = cx - radius_px`, `px1 = cx + radius_px` (clamp to image).
5. `chunk_dimensions()` → `(tw, th)`. `tiles_across = ceil(image_width / tw)`.
6. Tile bbox: `tc0 = px0 / tw`, `tc1 = px1 / tw`, same for rows.
7. Allocate output `Vec<f32>` of size `(px1-px0) × (py1-py0)`, filled with `nodata`.
8. For each tile `(tr, tc)` in the tile bbox:
   - `index = tr * tiles_across + tc`
   - `read_chunk(index)` → tile pixels as `Vec<f32>`
   - Compute overlap of tile pixel rect with output window
   - Copy overlapping rows/columns into output buffer
9. Build `Heightmap` with correct `crs_origin_*`, `dx_meters`, `dy_meters`, `crs_epsg`.

### Key correctness detail

`read_chunk` returns a full `tile_w × tile_h` buffer even for partial edge tiles
(padded with zeros/nodata). Use `chunk_data_dimensions(index)` to get the actual
valid pixel count for edge tiles if needed — or just treat zero-padding as nodata.

### Parser CRS routing (unchanged from current code)

- `scale >= 5.0` → EPSG:31287 — 5m DGM
- `scale >= 1.0` → EPSG:3035 — 1m LiDAR
- `scale < 1.0` → EPSG:4326 — 30m Copernicus (full-tile load via `load_grid`, not `extract_window`)

`extract_window` is only called for the 5m and 1m tiers.

---

## What to measure after each part

### After `crop`

- AO compute time on 667×667 crop vs 10800×10800 full grid
- Visual quality: does the AO boundary ring (where `1.0` fill meets computed AO) look acceptable?

### After `extract_window`

- Tile read time for a 2000×2000 window from the 5m file (cold vs warm cache)
- Compare vs `gdal_translate -projwin` shell-out for same window (baseline)
- Normals/shadows/AO compute time for a 2000×2000 Heightmap
