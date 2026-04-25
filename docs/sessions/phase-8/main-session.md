# Phase 8 — Session Log

---

## 2026-04-25

### What we worked on

More DEM tile extraction and viewer quality-of-life improvements. Confirmed `hintertux_8km_1m.tif`
renders correctly. Extracted additional 1m LiDAR tiles. Added camera-out-of-bounds behaviour
(tried shader fix, reverted due to fps cost — kept original hard break).

### Codebase changes this session

#### `src/viewer/mod.rs`
- `geotiff_is_projected` replaced by `geotiff_pixel_scale` — returns raw scale value so
  dispatcher can distinguish EPSG:3035 (scale=1.0) from EPSG:31287 (scale≥5.0) from geographic
- Auto-dispatch: `scale >= 5.0` → `parse_geotiff_epsg_31287`; `scale >= 1.0` → `parse_geotiff_epsg_3035`;
  else → `parse_geotiff`
- Camera named position updated: 47°04'34.36"N 11°41'15.33"E, 3258m

#### `crates/render_gpu/src/shader_texture.wgsl`
- Tried replacing bounds `break` with `in_bounds` guard + 5× step outside tile — terrain stays
  visible when camera exits map. Reverted: every out-of-bounds ray now marches to `t_max`,
  fps drop too large. Original hard break restored.

### Tiles extracted this session

| File | Source | CRS | Size | Resolution |
|---|---|---|---|---|
| `tiles/big_size/hintertux_8km_1m.tif` | BEV 1m LiDAR | EPSG:3035 | 8001×8001 | 1m |
| `tiles/big_size/hintertux_shifted_8km_1m.tif` | BEV 1m LiDAR | EPSG:3035 | 8001×8001 | 1m (4km south) |
| `tiles/big_size/salz_east_to_tux_base_8km_1m.tif` | BEV 1m LiDAR (east tile) | EPSG:3035 | 8001×8001 | 1m |

GDAL commands:
```sh
# 8km patch, original position
gdal_translate -projwin 4442000 2667978 4450000 2659978 -of GTiff \
  tiles/big_size/1m_innsbruck_area/CRS3035RES50000mN2650000E4400000.tif \
  tiles/big_size/hintertux_8km_1m.tif

# 8km patch, shifted 4km south
gdal_translate -projwin 4442000 2663978 4450000 2655978 -of GTiff \
  tiles/big_size/1m_innsbruck_area/CRS3035RES50000mN2650000E4400000.tif \
  tiles/big_size/hintertux_shifted_8km_1m.tif

# 8km patch from eastern neighbour tile (Salzburg south)
gdal_translate -projwin 4450000 2667978 4458000 2659978 -of GTiff \
  tiles/big_size/1m_salzburg_south_area/CRS3035RES50000mN2650000E4450000.tif \
  tiles/big_size/salz_east_to_tux_base_8km_1m.tif
```

### Key facts / lessons

- wgpu texture dimension limit 8192: 10001×10001 fails, 8001×8001 passes
- Out-of-bounds ray fix (skip bounds break, use `in_bounds` guard): visually correct but
  fps-prohibitive — all out-of-bounds rays march to t_max with no early exit. Hard break
  is the correct trade-off when camera stays near tile boundaries.
- Camera named position (WGS84) → `latlon_to_tile_metres` dispatches on `crs_epsg` field,
  works correctly for all three CRS types (4326, 31287, 3035)

### Open items

- Viewer feature items from roadmap still pending: shadow toggle (`.`), fog toggle (`,`),
  VAT presets (`;`), LOD presets (`'`)
- Measure normals/shadow/AO startup time for 8001×8001 vs 3601×3601

---

## 2026-04-20

### What we worked on

Phase 8 Part 0: Higher-resolution DEM data integration. Worked through three progressively
finer data sources: Copernicus GLO-30 (~30m, already done in prior session), BEV Austria
DGM 5m (EPSG:31287), and BEV Austria 1m LiDAR (EPSG:3035).

---

### Codebase changes this session

#### `Heightmap` struct generalisation (from previous session, confirmed working)
- `data: Vec<i16>` → `Vec<f32>`, `nodata: i16` → `f32`
- New fields: `crs_origin_x: f64`, `crs_origin_y: f64`, `crs_epsg: u32`
  - Geographic (EPSG:4326): `crs_origin_x = origin_lon`, `crs_origin_y = origin_lat`, `crs_epsg = 4326`
  - Austria Lambert (EPSG:31287): raw tiepoint easting/northing, `crs_epsg = 31287`
  - LAEA Europe (EPSG:3035): raw tiepoint easting/northing, `crs_epsg = 3035`

#### `crates/dem_io/src/geotiff.rs`
- `geotiff_pixel_scale(path)` — reads ModelPixelScaleTag[0]; `< 0.1` = geographic, `>= 1.0` = projected
- `parse_geotiff` — EPSG:4326 (GLO-30), `crs_epsg = 4326`
- `parse_geotiff_epsg_31287` — BEV DGM 5m Austria Lambert; approximate WGS84 from LCC false origin
- `parse_geotiff_epsg_3035` — BEV 1m LiDAR LAEA Europe; `Limits::unlimited()` for large files;
  `laea_epsg3035_inverse()` for approximate WGS84 from spherical LAEA inverse (~100m accuracy)

#### `src/viewer/mod.rs`
- **Auto-dispatch by scale**: `scale >= 5.0` → EPSG:31287, `scale >= 1.0` → EPSG:3035, else geographic
- **Lambert Conformal Conic forward** `lcc_epsg31287(lat, lon)` — Bessel 1841 ellipsoid, full formula
- **LAEA forward** `laea_epsg3035(lat, lon)` — spherical approximation, consistent with inverse
- **`latlon_to_tile_metres(lat, lon, hm)`** — dispatches on `hm.crs_epsg`; returns `None` if outside tile
- **Named camera position**: `CAM_LAT = 47.076211`, `CAM_LON = 11.687592` (47°04'34.36"N 11°41'15.33"E),
  `CAM_ELEV = 3258.0m` — Hintertux glacier; works for any tile containing that point
- **Camera init**: `latlon_to_tile_metres` → tile-local metres; falls back to `[2457*dx, 3328*dy, 3341]`
  if point is outside tile

#### `crates/render_gpu/src/context.rs`
- `DeviceDescriptor::default()` → `required_limits: adapter.limits()` — requests full hardware
  limits; fixes "max_storage_buffer_binding_size 134217728 exceeded" for 8001×8001 tiles

---

### DEM tiles extracted this session

| File | Source | CRS | Size | Resolution |
|---|---|---|---|---|
| `tiles/Copernicus_DSM_COG_10_N47_00_E011_00_DEM/` | Copernicus GLO-30 | EPSG:4326 | 3601×3601 | ~30m |
| `tiles/big_size/hintertux_5m.tif` | BEV DGM 5m | EPSG:31287 | 2001×2001 | 5m |
| `tiles/big_size/hintertux_18km_5m.tif` | BEV DGM 5m | EPSG:31287 | 3600×3600 | 5m |
| `tiles/big_size/hintertux_3km_1m.tif` | BEV 1m LiDAR | EPSG:3035 | 3600×3600 | 1m |
| `tiles/big_size/hintertux_8km_1m.tif` | BEV 1m LiDAR | EPSG:3035 | 8001×8001 | 1m |
| `tiles/big_size/hintertux_10km_1m.tif` | BEV 1m LiDAR | EPSG:3035 | 10001×10001 | 1m (too large) |

GDAL extraction commands used:
```sh
# BEV DGM 5m — 5km patch
gdal_translate -projwin 268605 361962 278605 351962 -of GTiff tiles/big_size/5m_whole_austria/DGM_R5.tif tiles/big_size/hintertux_5m.tif

# BEV DGM 5m — 18km patch (same pixel count as GLO-30)
gdal_translate -projwin 264605 365962 282605 347962 -of GTiff tiles/big_size/5m_whole_austria/DGM_R5.tif tiles/big_size/hintertux_18km_5m.tif

# BEV 1m LiDAR — 8km patch (fits wgpu 8192 texture limit)
gdal_translate -projwin 4442000 2667978 4450000 2659978 -of GTiff tiles/big_size/1m_innsbruck_area/CRS3035RES50000mN2650000E4400000.tif tiles/big_size/hintertux_8km_1m.tif
```

Hintertux camera centre in each CRS:
- WGS84: 47°04'34.36"N, 11°41'15.33"E (= 47.076211°N, 11.687592°E)
- EPSG:31287: easting=273605, northing=356962 (via `gdaltransform`)
- EPSG:3035: easting=4449262, northing=2663978 (via `gdaltransform`)

---

### Key technical facts learned

- **EPSG:3035 (LAEA Europe)**: Lambert Azimuthal Equal Area; centre lat0=52°N, lon0=10°E;
  FE=4321000, FN=3210000; GRS 1980 ellipsoid; scale tag gives metres directly
- **EPSG:31287 (Austria Lambert)**: Lambert Conformal Conic; Bessel 1841 ellipsoid;
  false origin 47.5°N 13.333°E; FE=400000, FN=400000; 5m/pixel for BEV DGM
- **`tiff` crate default limit**: rejects images with total bytes > ~128 MB; fix with
  `Decoder::with_limits(Limits::unlimited())`
- **wgpu default `max_storage_buffer_binding_size`**: 128 MB; hardware supports much more;
  fix with `required_limits: adapter.limits()` in `DeviceDescriptor`
- **wgpu texture dimension limit**: 8192 per axis; 10001×10001 tile fails; 8001×8001 passes
- **BEV DGM 5m NoData**: sentinel value is `0.0` (not NaN, not -9999); safe to use because
  minimum valid Austrian elevation >> 0
- **Camera positioning**: store named positions as WGS84 lat/lon; forward-project at runtime
  to tile-local metres; falls back gracefully if point outside tile

---

### Open items / next steps

- Run `hintertux_8km_1m.tif` in viewer — session ended before confirming it renders correctly
- The 1m tile normals/shadow/AO computation time will be much longer than 5m — measure it
- Consider: for very large tiles, AO computation (`compute_ao_true_hemi`) may need parallelism tuning
- GLO-10 (EEA-10): confirmed closed access — skip
- Part 0 viewer items still pending from `viewer-phase-8.md`:
  - Shadow toggle (`.` key)
  - Fog toggle (`,` key)
  - Visual Artifact Tolerance presets (`;`)
  - LOD Distance presets (`'`)
  - Out-of-core tile streaming (deferred to later)
