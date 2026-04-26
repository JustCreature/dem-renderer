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
