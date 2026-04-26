# Multi-Tile Multi-Resolution Terrain Loading

Replaces the original Plan 5 (chunk-based LRU texture cache) with a design that matches the
actual data sources: SRTM 30m full tiles, BEV DGM 5m regional file, BEV LiDAR 1m 50km sheets.

---

## Architecture Overview

Three concentric resolution tiers are active simultaneously:

| Tier   | Source             | Pixel scale | Visible radius (configurable) | Update trigger |
|--------|--------------------|-------------|-------------------------------|----------------|
| 30m    | SRTM GeoTIFF tiles | ~30 m/px    | unlimited (background)        | tile boundary crossing |
| 5m     | BEV DGM (Austria)  | 5 m/px      | default 10 km                 | drift > 40% of radius |
| 1m     | BEV LiDAR sheets   | 1 m/px      | default 5 km                  | drift > 40% of radius |

The GPU holds three heightmap textures at any time, one per tier. The shader samples the finest
resident tier at each ray position, blending across tier boundaries with a configurable lerp zone.

One background loader thread per tier manages extraction and GPU upload independently. If the
camera moves faster than a tier can reload, the next coarser tier is rendered as fallback until
the new window is ready.

---

## Step 1 — 30m Sliding 3×3 Window

**Goal:** load nine 30m tiles (a 3×3 grid centred on the camera's current tile) and present
them to the GPU as a single seamless texture. When the camera crosses a tile boundary the grid
slides: the camera's tile becomes the new centre, three tiles on the trailing edge are dropped,
three new tiles on the leading edge are loaded.

### Why 3×3 and not 2×2

At 47°N, SRTM tiles are not square:
- N-S: 1° latitude ≈ **111 km**
- E-W: 1° longitude × cos(47°) ≈ **76 km**

Fog distance is 60 km. From the centre of the camera's tile:
- To the E/W edge: 38 km — fog overshoots by **22 km**, always. E/W neighbours are mandatory.
- To the N/S edge: 55.5 km — fog overshoots by only **4.5 km** from dead centre.

From any point in the southern half of the tile the southern neighbour is within fog distance;
symmetric for the northern half. A fixed 3×3 covers all cases without conditional loading logic.

### Sliding policy

The camera is **always in the centre tile**. Tile identity is the integer-degree origin
(e.g. N47E011). When `floor(camera_lon)` or `floor(camera_lat)` changes, the centre tile
changes → slide the grid. No midpoint arithmetic needed; the tile boundary crossing is the
trigger.

On a slide:
- **Drop** the 3 tiles on the trailing edge.
- **Load** 3 new tiles on the leading edge (disk read + CPU preprocessing).
- **Keep** the 6 tiles that remain in the new 3×3 footprint untouched.

### Design

- Keep nine `Option<Heightmap>` handles in a `[[Option<Heightmap>; 3]; 3]` grid indexed
  `[row][col]`, row 0 = northernmost, col 0 = westernmost.
- Assemble into one flat `Vec<f32>` (3 × 3601 − 2 seam pixels per join = 10801 × 10801)
  whenever the grid changes; upload as a single R16Float GPU texture (fits within 8192 px limit
  — 10801 exceeds it; see note below).
- **Seam at tile edges**: adjacent SRTM/Copernicus tiles share edge pixels. Drop the duplicate
  edge column/row when stitching (col 3600 of tile N == col 0 of tile N+1 → assembled width =
  3 × 3601 − 2 = 10801).
- **Texture dimension limit (8192 px)**: 10801 > 8192 — a single assembled texture won't fit.
  Two options: (a) keep three separate 1×3601 column textures and sample with an offset in the
  shader; (b) downsample the outer 8 tiles to half resolution (1801 px) before assembling,
  giving 3 × 1801 − 2 = 5401 px — fine since outer tiles are only used beyond ~38 km where
  30m detail is invisible anyway. **Prefer option (b): outer tiles at half-res.**
- **CRS**: all 30m tiles are EPSG:4326. Tile identification by integer-degree origin.
  Camera WGS84 position → `(floor(lon), floor(lat))` = centre tile origin.

### Preprocessing per tile

Full preprocessing (normals + shadows + AO) for all 9 tiles is expensive (~9× centre-tile
cost). Strategy:
- **Centre tile**: full normals + shadows + AO (same as today).
- **8 outer tiles**: normals only (scalar, single-thread); skip shadows and AO.
  Shadows and AO are only visually important at close range; beyond ~38 km the 30m tier
  is blended with the 5m tier anyway.

### What to measure

- Time to assemble 9 half-res outer tiles + 1 full-res centre tile into the stitched buffer.
- Time to upload the stitched texture to GPU (`write_texture`).
- Visual seam quality at tile boundaries (normal discontinuity at the centre/outer boundary
  due to different preprocessing — expect a faint ring; quantify it).
- Tile slide latency: time from boundary crossing to new grid visible on screen.

### Open questions

- Which Copernicus tiles are needed for the Hintertux 3×3 grid? Centre = N47E011; need
  N46E010, N46E011, N46E012, N47E010, N47E012, N48E010, N48E011, N48E012.
- Are all 9 tiles available from the Copernicus GLO-30 AWS bucket for this region?

---

## Step 2 — Windowed GeoTIFF Extraction (Shared Infrastructure)

**Goal:** in-process equivalent of `gdal_translate -projwin`. Given a source GeoTIFF path, a
centre point in the file's native CRS, and a radius in metres, return a `Heightmap` covering
that window — without loading the entire file.

### Design

- Use the `tiff` crate's scanline API: seek to the first row of the window, read only the needed
  rows. Column cropping applied per-row by slicing the decoded scanline.
- CRS coordinate → pixel row/col: already implemented as the inverse of the forward projections
  in `geotiff.rs`. Use `ModelTiepointTag` + `ModelPixelScaleTag` for the affine transform.
- Output: a `Heightmap` with correct `crs_origin_*`, `crs_epsg`, `dx_meters`, `dy_meters`.
- **Multi-source-tile stitching**: when the requested window crosses the boundary of the current
  source file, identify the adjacent source tile(s) by their filename convention
  (`CRS3035RES50000mN{Y}E{X}.tif` for 1m, the single Austria DGM file for 5m), extract the
  complementary strip(s) from each, and assemble into one `Heightmap`.

### Interface (conceptual)

```
fn extract_window(
    sources: &[SourceTile],   // one or more open source files covering the area
    centre_crs: (f64, f64),   // easting, northing in native CRS
    radius_m: f64,
    crs_epsg: u32,
) -> Heightmap
```

### What to measure

- Scanline-read throughput for a 2000×2000 window from a 50,000×50,000 source file.
- Does seeking to the correct row cost more than sequential read? (Depends on TIFF strip layout.)
- Compare in-process extraction time vs shelling out to `gdal_translate`.

---

## Step 3 — Per-Tier Background Loader Threads

**Goal:** each tier has one dedicated background thread that monitors camera position and
re-extracts its window when the camera drifts beyond the reload threshold.

### Design

- **One thread per tier** (30m / 5m / 1m; extensible to 90m or other resolutions later).
- Each thread holds: current window centre, current `Heightmap`, open source file handle(s).
- **Reload threshold**: `drift > reload_fraction * radius_m`, default `reload_fraction = 0.4`.
  Example: 5m tier with 10 km radius → reload after 4 km drift from window centre.
- **Communication**: main thread sends camera position updates via a channel (bounded, size 1 —
  drop stale positions). Loader thread sends back a `(Heightmap, normals, shadows, ao)` bundle
  via another channel when extraction + CPU preprocessing completes.
- **Fallback rendering**: if the main thread polls the channel and finds no new tile ready
  (camera moved faster than loader), continue rendering the previous tile for that tier.
  The shader's distance-based tier selection already provides a coarser fallback automatically.
- **GPU upload**: happens on the main thread (wgpu `write_texture` must run on the device thread)
  immediately after the bundle arrives on the channel.

### Configurable parameters (viewer CLI / runtime)

```
--radius-5m <metres>      default: 10000
--radius-1m <metres>      default: 5000
--reload-fraction <0..1>  default: 0.4
```

### AO radius optimisation (implement here)

AO (`compute_ao_true_hemi`) on the full 10800×10800 assembled grid takes ~7.8s on Intel Mac
(116M pixels × 16 DDA sweeps). Beyond ~10km AO is imperceptible at 30m resolution.

When wiring up the background loader, limit AO to a cropped window around the camera:
1. Add `crop(hm, row_start, col_start, rows, cols) -> Heightmap` to `dem_io::grid`
2. Crop a ~10km radius (~667×667 px at 30m/px) centred on the camera position
3. Run `compute_ao_true_hemi` on the cropped `Heightmap`
4. Embed the result into a full-size AO buffer (rest filled with `1.0`)
5. On each background reload, recompute AO for the new camera-centred crop

Expected speedup: 116M → 0.4M pixels = ~290×, dropping AO from ~7.8s to ~27ms per reload.

### What to measure

- Latency from camera crossing reload threshold to new tile visible on screen.
- CPU preprocessing time (normals + shadows + AO) for a 2000×2000 window vs 8001×8001.
- Whether the loader thread causes frame time spikes (check with GPU timestamp queries or
  Instruments) when the GPU upload fires.

---

## Step 4 — Multi-Source-Tile Stitching

**Goal:** handle the case where the camera's extraction window crosses the boundary of one BEV
source file into an adjacent one.

### Design

- **Source tile registry**: a list of `(path, bbox_crs)` entries covering the region of interest.
  For 1m LiDAR the bbox is derivable from the filename (`CRS3035RES50000mN{Y}E{X}`). For 5m
  the single DGM file covers all of Austria.
- **Window decomposition**: given a requested window bbox, find all source tiles whose bbox
  intersects it. Extract the relevant sub-rectangle from each, then assemble into one flat
  array with correct row/col offsets.
- **Edge alignment**: source tiles share no duplicate edge pixels (unlike SRTM). Adjacent pixels
  are simply adjacent in the stitched buffer.
- **Missing data**: if no source tile covers part of the window (e.g. near the Austrian border),
  fill with the 30m tier's resampled height as a stand-in.

### What to measure

- Does a stitched window from two source tiles have a visible seam in normals/shadows?
  (Expected: no, because heights are continuous — seams only appear if the two files use
  different datums or processing pipelines.)

---

## Step 5 — Multi-Tier Shader

**Goal:** the shader holds three heightmap textures (30m / 5m / 1m). At each ray position it
samples the finest tier whose window covers that point, blending across tier boundaries.

### Design

- **Three uniform fields**: `radius_5m_m: f32`, `radius_1m_m: f32` in `CameraUniforms`.
- **Tier selection**: compute horizontal distance from camera position to ray hit point.
  - `dist < radius_1m * blend_inner` → pure 1m sample
  - `radius_1m * blend_inner < dist < radius_1m * blend_outer` → lerp(1m, 5m)
  - `dist < radius_5m * blend_inner` → pure 5m sample
  - `radius_5m * blend_inner < dist < radius_5m * blend_outer` → lerp(5m, 30m)
  - `dist > radius_5m * blend_outer` → pure 30m sample
- **Blend zone width**: configurable, default 10% of the tier radius
  (e.g. 1m tier with 5 km radius → blend starts at 4.5 km, ends at 5 km).
- **UV mapping per tier**: each tier has its own origin + scale stored in uniforms;
  `uv = (world_xy - tier_origin) / tier_extent` clamps to [0,1].
- **Fallback when tier not yet loaded**: `radius_1m = 0.0` signals "tier not resident";
  shader skips that tier entirely.

### What to measure

- Visual quality of the lerp blend zone — does the seam disappear?
- FPS impact of three texture samples per ray step vs one (expect < 5% on M4 SLC-cached data).
- Whether normals/shadows from different tiers produce visible discontinuities at blend zone
  edges (they will if the underlying height data disagrees significantly).

---

## Implementation Order

1. Step 1 — 30m sliding window (no background thread yet; synchronous swap on midpoint crossing)
2. Step 2 — windowed GeoTIFF extraction function (unit-testable, no threading)
3. Step 3 — wrap Step 2 in per-tier background threads; integrate with viewer
4. Step 4 — multi-source-tile stitching (extend Step 2's `extract_window`)
5. Step 5 — multi-tier shader (add tier uniforms, blend logic)

Each step produces a working, measurable result before the next begins.

---

## Open Items / Deferred

- 90m tier (Copernicus GLO-90 or SRTM 3 arc-second): architecture already supports it —
  add a fourth tier thread and a fourth texture binding.
- Out-of-core streaming for very large single tiles (> 8192 px): would require the original
  chunk-based LRU cache from the old Plan 5. Not needed until tile dimensions exceed 8192.
- GPU-side tile cache (hardware sparse textures): wgpu does not expose VkSparseBinding or
  Metal sparse textures; deferred until wgpu adds support or we drop to raw Metal/Vulkan HAL.
- Texture dimension fallback: if `max_texture_dimension_2d < 10800` (e.g. Asus Pentium N3700
  with integrated GPU), downsample the assembled 10800×10800 buffer to 8192×8192 using a box
  filter before upload. Loses ~25% linear resolution but renders correctly on all hardware.
