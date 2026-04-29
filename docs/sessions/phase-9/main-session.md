# Phase 9 — Session Log

---

## 2026-04-25 (session 1)

### What we worked on

Phase 9 opened. Carried all multi-tile multi-resolution streaming work forward from Phase 8.
No implementation yet — planning complete, ready to begin Step 1.

### Open items at phase start

- Download 8 surrounding Copernicus GLO-30 tiles for Hintertux 3×3 grid:
  N46E010, N46E011, N46E012, N47E010, N47E012, N48E010, N48E011, N48E012
- Step 1: 30m 3×3 sliding window
- Step 2: windowed GeoTIFF extraction
- Step 3: per-tier background loader threads
- Step 4: multi-source-tile stitching
- Step 5: multi-tier shader

See `docs/planning/multi-tile-multiple-resolution-load.md` for full plan.

---

## 2026-04-25 (session 2)

### What we worked on

**Architecture decisions:**
- Settled on 9 separate GPU textures (one per tile slot) instead of assembled single texture,
  then revised: single assembled texture is fine since shader uses `hm_cols`/`hm_rows` dynamically
- Rejected downsampled outer tiles in favour of full-res 30m for all 9 — avoids corner-case
  quality drop when camera is near tile boundary looking outward
- Decided against GLO-90 for outer tiles for now — keep single data source (GLO-30 everywhere)
- Added texture dimension fallback note to plan: if `max_texture_dimension_2d < 10800`,
  downsample assembled buffer to 8192×8192 at load time

**Data layer exploration:**
- Confirmed Copernicus GLO-30 tiles are 3600×3600, pixel-is-area (not 3601 pixel-is-point)
- Origin offset: pixel centres at ±0.5/3600° from integer degree boundary
- Adjacent tiles abut perfectly — difference between last pixel of tile N and first pixel of
  tile N+1 is exactly 1/3600° (one pixel spacing). Simple concatenation, no deduplication.
- 3×3 assembled grid = 10800×10800 px

**Downloaded 8 surrounding tiles:**
N46E010, N46E011, N46E012, N47E010, N47E012, N48E010, N48E011, N48E012

**Implemented `crates/dem_io/src/grid.rs`:**
- `assemble_grid(&[[Option<&Heightmap>; 3]; 3]) -> Heightmap` — row-interleaved assembly,
  None tiles filled with 0.0, NW tile provides origin/scale metadata
- `load_grid<F>(tiles_dir, centre_lat, centre_lon, loader) -> Heightmap` — constructs 9 tile
  paths from Copernicus naming convention, loads via injectable `loader` closure
- `tile_path(tiles_dir, lat, lon) -> PathBuf` — helper for Copernicus filename convention

**Wired into viewer:**
- `prepare_scene` now calls `load_grid` for EPSG:4326 GLO-30 tiles (scale < 1.0)
- `parse_copernicus_lat_lon(tile_path)` parses centre lat/lon from directory name —
  no more hardcoded `47, 11`
- Added startup timing instrumentation

**Startup timing (Intel Mac, 10800×10800):**
- load_grid (disk, 9 × DEFLATE COG): 4.52s
- normals: 185ms
- shadows: 525ms
- AO: 7.81s  ← dominates; deferred optimisation noted in Step 3 of plan

**Verified rendering correct:**
- All 9 tiles render seamlessly — no seam artifacts at tile boundaries
- Corner case (camera at SE corner looking SE) works correctly
- Shader UV math already dynamic via `cam.hm_cols`/`cam.hm_rows` — no shader changes needed

**Plan updates:**
- Added AO radius optimisation note to Step 3 (crop to ~10km radius, ~290× speedup)
- Added texture dimension fallback item to Open Items / Deferred

### Open items remaining

- Step 1 complete (static 3×3 grid, synchronous load)
- Step 2: windowed GeoTIFF extraction (`extract_window` function)
- Step 3: per-tier background loader threads (+ AO radius crop)
- Step 4: multi-source-tile stitching
- Step 5: multi-tier shader
- Sliding window / tile boundary crossing detection (not yet implemented)

---

## 2026-04-25 (session 3)

### What we worked on

**Completed sliding window implementation (Step 1 fully done):**

**Inverse projection — `tile_meters_to_latlon_epsg_4326`:**
- Added to `viewer/mod.rs` alongside the existing `latlon_to_tile_metres`
- For EPSG:4326: `lon = crs_origin_x + (cam_x / dx_meters) * dx_deg`,
  `lat = crs_origin_y - (cam_y / dy_meters) * dy_deg.abs()`
- Returns `(lat, lon)` — used every frame to detect tile crossing

**`GpuScene::update_heightmap` implemented:**
- `_ao_texture: wgpu::Texture` added to `GpuScene` struct (was dropped before, preventing re-upload)
- Normal buffers (`_nx_buf`, `_ny_buf`, `_nz_buf`) gained `COPY_DST` usage flag — required for `write_buffer`
- Mip generation extracted to free function `write_hm_mips(queue, texture, base_data, cols, rows)`,
  called from both `new()` and `update_heightmap()`
- `update_heightmap(&mut self, hm, normals, ao)` now correctly re-uploads all 4 resources
  (hm texture + 7 mips, nx/ny/nz buffers, ao texture) and updates `hm_cols`/`hm_rows`/`dx_meters`/`dy_meters`

**`TileBundle` + background tile loader thread:**
- `struct TileBundle { hm, normals, ao }` in viewer
- `tile_tx: SyncSender<(i32,i32)>`, `tile_rx: Receiver<TileBundle>`, `tile_loading: bool` on `Viewer`
- Tile loader thread: `load_grid` + `compute_normals_vector_par` + `compute_ao_true_hemi` → sends bundle
- SyncSender capacity 1 — stale requests dropped automatically

**Per-frame sliding window logic in `RedrawRequested`:**
- Crossing detection: `tile_meters_to_latlon_epsg_4326` → `floor()` → compare against `centre_lat/lon`
- On bundle receive: re-project `cam_pos` into new grid via `latlon_to_tile_metres`, call
  `update_heightmap`, respawn shadow worker with new `Arc<Heightmap>`, update `centre_lat/lon`
- Old shadow worker exits automatically when its sender (`shadow_tx`) is replaced and dropped
- Prints `"loading tile N47E012"` and `"tile slide complete: N47E012"` on crossing

**Bugs found and fixed:**
- `dt` was near-zero: restructured `RedrawRequested` so tile sliding (needs `&mut scene`) happens
  before `let scene: &GpuScene` immutable borrow; `dt = last_frame.elapsed()` before resetting
  `last_frame` so it measures true inter-frame time
- Shadow recomputing every frame: added `last_shadow_az`/`last_shadow_el` to Viewer; only
  recomputes when sun moves ≥ 0.1° (0.00175 rad) — eliminates 466 MB GPU upload every ~0.5s

**Download script updated:**
- `download_copernicus_tiles_30m.sh` now loops lat 45–49, lon 9–13 (5×5 = 25 tiles)
- Uses `printf` for correct zero-padded naming (`N45`, `E009`)
- Skips tiles already present (`-f "$DEST"` check)
- Downloads 16 new outer tiles, skips existing 9 inner ones

### Open items remaining

- Step 2: windowed GeoTIFF extraction (`extract_window` function)
- Step 3: per-tier background loader threads (+ AO radius crop)
- Step 4: multi-source-tile stitching
- Step 5: multi-tier shader
- Download and test 5×5 tile grid (outer 16 tiles pending)

---

## 2026-04-26 (session 4)

### What we worked on

**COG file layout analysis (tiffinfo):**
- GLO-30 30m: 3600×3600, Tile 1024×1024, 4 IFD levels (1800/900/450), AdobeDeflate, f32, EPSG:4326
- BEV DGM 5m: 120001×70001, Tile 256×256, 9 IFD levels, LZW, f32, EPSG:31287
- LiDAR 1m: 50001×50001, Tile 256×256, 8 IFD levels, LZW, f32, EPSG:3035
- Key insight: GLO-30 overview at IFD 1 = 1800×1800 (half-res outer tiles for free, no software downsampling)
- `tiff` crate v0.11.3 API confirmed: `seek_to_image(ifd)`, `chunk_dimensions()`, `tile_count()`, `read_chunk(index)`

**Parser CRS routing clarified:**
- `scale >= 5.0` → `parse_geotiff_epsg_31287` (5m DGM, EPSG:31287)
- `scale >= 1.0` → `parse_geotiff_epsg_3035` (1m LiDAR, EPSG:3035)
- `scale < 1.0` → `load_grid` + `parse_geotiff` (30m GLO-30, EPSG:4326)

**Step 2 split clarified:**
- `crop(hm, row_start, col_start, rows, cols) -> Heightmap` — in-memory slice for 30m AO only
- `extract_window(path, centre_crs, radius_m, ifd_level) -> Heightmap` — tile-based disk read for 5m/1m tiers
- Planning doc saved to `docs/planning/tmp/crop_extract.md`

**`crop` implemented in `crates/dem_io/src/grid.rs`:**
- Row-by-row slice copy; updates `origin_lat/lon`, `crs_origin_x/y` by pixel offset × scale
- Exported from `dem_io::lib` via `pub use grid::{assemble_grid, crop, load_grid}`

**AO crop optimization wired in (`src/viewer/mod.rs`):**
- `prepare_scene` now takes `cam_lat: f64, cam_lon: f64`; `run()` passes `CAM_LAT`/`CAM_LON`
- Tile loader channel upgraded to `(i32, i32, f64, f64)` — tile lat/lon + camera WGS84 lat/lon
- `compute_ao_cropped(hm, cam_lat, cam_lon) -> Vec<f32>` extracted as free function (deduplicates two identical blocks)
- `AO_RADIUS_M = 20_000.0` module-level constant (was 10_000, increased so camera doesn't exit AO zone before tile slide)
- AO timing: **290ms** (Intel Mac) vs 7.81s before — **27× speedup**; pixel reduction 116M → ~1.78M (20km radius)

**Known limitation identified:**
- AO crop is centred on the camera position at tile-crossing time (tile boundary)
- As camera moves inward into new tile, it exits the 20km AO window → `1.0` fill visible
- Fix: Step 3 drift-based recompute — `ao_tx/ao_rx` separate channel, recompute when drift > 10km
- Architecture: AO-only recompute channel separate from tile-slide channel (different triggers)

### Open items remaining

- Step 2: `extract_window` — tile-based selective disk read for 5m/1m tiers
- Step 3: drift-based AO recompute (`ao_tx: SyncSender<(f64,f64)>`, `ao_rx: Receiver<Vec<f32>>`,
  `last_ao_center: (f64,f64)` on Viewer, threshold = AO_RADIUS_M * 0.5)
- Step 3: per-tier background loader threads for 5m and 1m
- Step 4: multi-source-tile stitching
- Step 5: multi-tier shader

---

## 2026-04-26 (session 5)

### What we worked on

**Implemented `extract_window` in `crates/dem_io/src/geotiff.rs`:**
- Signature: `extract_window(path, centre_crs: (f64,f64), radius_m, ifd_level, crs_epsg) -> Result<Heightmap, DemError>`
- `seek_to_image(ifd_level)` before `dimensions()` — critical for correct IFD-level dimensions
- Tags (ModelPixelScaleTag 33550, ModelTiepointTag 33922) read after seek; `dx_meters = scale[0]`, `dy_meters = scale[1]`, `crs_origin_x = tiepoint[3]`, `crs_origin_y = tiepoint[4]`
- Affine inverse: `cx = (centre_crs.0 - crs_origin_x) / dx_meters`, `cy = (crs_origin_y - centre_crs.1) / dy_meters`
- CRS dispatch via `crs_epsg` param: 3035 → `laea_epsg3035_inverse`, 31287 → `laea_epsg31287_inverse` (new helper extracted from hardcoded lines in `parse_geotiff_epsg_31287`)
- Pixel bbox: `px0/px1/py0/py1` clamped to image bounds; `out_w = px1-px0`, `out_h = py1-py0`
- Tile bbox: `tc0 = px0/tw`, `tc1 = (px1+tw-1)/tw` (exclusive, rounded up), same for rows
- Tile loop: `read_chunk(tr * tiles_across + tc)` → row-by-row overlap copy into output buffer
- Overlap copy: `src = (row - tile_row0) * tw + (col_start - tile_col0)`, `dst = (row - py0) * out_w + (col_start - px0)`, `len = col_end - col_start`
- Output `Heightmap`: `crs_origin_x = file_origin_x + px0 * dx_meters`, `crs_origin_y = file_origin_y - py0 * dy_meters`
- `laea_epsg31287_inverse` extracted as named private function (was inline in `parse_geotiff_epsg_31287`)
- `extract_window` exported from `dem_io::lib`

**Wired into `prepare_scene`:**
- `centre_crs = lcc_epsg31287(cam_lat, cam_lon)` — absolute EPSG:31287 easting/northing (NOT tile-local metres)
- Bug identified and fixed: `latlon_to_tile_metres` returns tile-local offsets, not absolute CRS coords; `extract_window` needs absolute coords → call forward projection directly
- `extract_window` called at startup for diagnostic output; result not yet used for rendering

**Known limitation noted:**
- GeoTIFF tags (ModelPixelScaleTag, ModelTiepointTag) stored in IFD 0 only; `seek_to_image(ifd_level > 0)` + `get_tag` may fail — safe for now since `ifd_level = 0` always passed

### Measured numbers

- `extract_window` (5m BEV DGM, 5km radius, cold): **18.6ms**
- Output: 1707×1454 px (asymmetric — one side clamped by file boundary), elevation 1398–3336m ✓
- ~64 internal 256×256 tiles read out of ~128,000 total (~0.05% of file)

### Open items remaining

- Step 3: drift-based AO recompute (`ao_tx: SyncSender<(f64,f64)>`, `ao_rx: Receiver<Vec<f32>>`,
  `last_ao_center: (f64,f64)` on Viewer, threshold = AO_RADIUS_M * 0.5)
- Step 3: per-tier background loader threads — 5m and 1m tiers using `extract_window`
- Step 4: multi-source-tile stitching
- Step 5: multi-tier shader

---

## 2026-04-28 (session 6)

### What we worked on

**Completed Step 3 (5m + 1m background loader threads) and Step 4 (1m tile stitching).**

**1m streaming tier — full implementation (Steps 3+4+5 for 1m):**

Architecture: three-tier streaming stack — base (~20m, 90km), close (5m, 20km), fine (1m, 3.5km).
All three tiers use the `StreamingTier` abstraction (tx/rx channels, `needs_reload`, `try_trigger`, `try_recv`, `invalidate`).

Files modified:

- `src/viewer/tiers.rs` — added `BEV_1M_RADIUS_M = 3500.0`, `BEV_1M_DRIFT_THRESHOLD_M = 1000.0`; added `fine: Option<StreamingTier>` to `BevBaseState`
- `src/viewer/geo.rs` — added `laea_epsg3035_inverse` (spherical LAEA inverse, for event loop: EPSG:3035 origin → WGS84); added `lcc_epsg31287_inverse` (iterative LCC inverse, Bessel 1841, converges <10 iters, for worker: EPSG:31287 → WGS84)
- `crates/render_gpu/src/camera.rs` — added 8 new fields to `CameraUniforms` (hm1m_origin_x/y, hm1m_extent_x/y, hm1m_cols/rows, _pad8/9); struct is now 256 bytes
- `crates/render_gpu/src/scene.rs` — added `_hm1m_*` fields (texture R16Float, sampler, 4 storage buffers nx/ny/nz/shadow, 6 metadata scalars); added bindings 16–21 to `render_bgl`, all bind groups, `new()` init (1×1 placeholder), `upload_hm1m()`, `set_hm1m_inactive()`; updated both CameraUniforms construction sites
- `crates/render_gpu/src/shader_texture.wgsl` — added hm1m fields to WGSL CameraUniforms; added binding declarations 16–21; added `fine_tier_edge_dist` helper; updated `sample_h_exact` to three-tier blend; added 1m normals/shadow blend block in shading
- `src/viewer/mod.rs` — added `find_1m_tile`, worker thread (EPSG:31287→WGS84→EPSG:3035 conversion, `extract_window`, normals+shadow), fine tier poll block, `--1m-tiles-dir` plumbing; on base reload: `set_hm1m_inactive()` + `fine.invalidate()`
- `src/main.rs` — `--1m-tiles-dir <path>` CLI arg parsed; `viewer::run` signature updated

**Coordinate conversion chain for 1m tier:**
- Worker receives EPSG:31287 → `lcc_epsg31287_inverse` → WGS84 → `laea_epsg3035` → (e3035, n3035) → `extract_window`
- Event loop receives hm with EPSG:3035 origin → `laea_epsg3035_inverse` → WGS84 → `lcc_epsg31287` → EPSG:31287 → subtract base tile CRS origin → tile-local offset for shader

**Edge-tile stride bug fixed in `extract_window`:**
- 1m tiles are 50001×50001 with 256×256 internal tiles; last column of tiles is only 81px wide (50001 mod 256 = 81)
- `tiff` crate returns 81×256=20736 elements for edge tiles; old code used `tw=256` as stride → index out of bounds
- Fix: `actual_tw = tile_col1.min(cols) - tile_col0`; use as stride instead of declared `tw`

**1m tile stitching (two adjacent tiles at EPSG:3035 easting 4450000):**
- `find_1m_tile(dir, e3035, n3035) -> Option<PathBuf>` replaced by `find_1m_tiles(dir, e3035, n3035, radius_m) -> Vec<PathBuf>` — returns all tiles whose 50km bounds overlap the ±3500m window
- Worker now captures `tiles_1m_dir` (not a fixed tile path); calls `find_1m_tiles` dynamically on every reload; spawns unconditionally when `--1m-tiles-dir` provided
- `stitch_1m_windows(windows, centre_e, centre_n, radius_m) -> Heightmap` — aligns each window using `crs_origin_x/y` and 1m pixel arithmetic, fills NODATA gaps, returns a complete 7000×7000 output; single-tile case passes through without stitching

**Available 1m tiles (EPSG:3035):**
- `tiles/big_size/1m_innsbruck_area/CRS3035RES50000mN2650000E4400000.tif` — easting [4400000, 4450000)
- `tiles/big_size/1m_salzburg_south_area/CRS3035RES50000mN2650000E4450000.tif` — easting [4450000, 4500000)
- Shared boundary at easting 4450000; both tiles same northing band [2650000, 2700000)

### Open items remaining

- Step 3 (5m background loader): already implemented in same session as 1m (both use `StreamingTier`)
- Step 5 (shader three-tier blend): complete — `fine_tier_edge_dist`, smoothstep blend in raymarcher + shading
- Remaining: drift-based AO recompute for GLO-30 mode (camera exits 20km window → 1.0 fill)
- Remaining: 1m tier has no AO (ao: vec![] in TierData) — acceptable for now
- Deferred: tile stitching at north/south boundaries (current tiles share same northing band)
