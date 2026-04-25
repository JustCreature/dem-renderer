# Viewer Phase 8 Plan

Incremental viewer improvements. Each item is self-contained.

---

## Part 0 — Higher-Resolution DEM Data Sources

**Goal:** progressively replace the current N47 E011 SRTM 1 arc-second (~30m) tile with higher-resolution
data, starting from the easiest format-compatible source and finishing with sub-metre LiDAR.
Each step is a standalone experiment: download → load → render → observe visual/performance delta.

**Current baseline:** `n47_e011_1arc_v3.bil` — SRTM v3, 3601×3601 samples, ~30m/pixel, ENVI BIL format,
WGS84 geographic coordinates, `i16` heights.

**Region:** N47 E011 = Austrian/Bavarian Alps (Inn valley / Karwendel range, near Innsbruck).

---

### Part 0.1 — Copernicus GLO-30 (~30m, cleaner than SRTM)

**Resolution:** ~30m (same pixel density as current SRTM, same grid size ~3600×3600)

**Why first:** same resolution as current data but better quality — void-filled with better algorithms,
fewer radar artefacts, smoother ridgelines. Cheap visual quality win with minimal engineering.
Also a good forcing function to add GeoTIFF reading support, which all later steps reuse.

**Format:** GeoTIFF (`.tif`), not BIL — `parse_bil` will not read it directly.

**Download:**
- AWS Open Data (free, no account): `s3://copernicus-dem-30m/Copernicus_DSM_COG_10_N47_00_E011_00_DEM/`
- Or via OpenTopography: opentopography.org → Global Datasets → Copernicus GLO-30

**What needs to change in the codebase:**
- Either: convert offline with GDAL — `gdal_translate -of EHdr input.tif output.bil` — then `parse_bil` works unchanged
- Or: write a minimal GeoTIFF reader (reads TIFF tags for width/height/geotransform, then raw `i16` or `f32` scanlines)
- GeoTIFF approach is more reusable for GLO-10 and LiDAR steps; GDAL convert is faster to try first

**Implicit assumptions in `parse_bil` that GLO-30 satisfies:**
- Geographic (degree) coordinates ✓
- `i16` or `f32` sample type ✓ (GLO-30 is `f32` in GeoTIFF; GDAL convert can force `i16`)
- Equirectangular `dx_meters = dx_deg * 111320 * cos(lat)` formula ✓

**Expected outcome:** visually nearly identical to current render, but smoother terrain surface.
Performance identical (same grid size).

---

### Part 0.2 — SRTM 1/3 Arc-Second (~10m)

**Resolution:** ~10m, 10800×10800 samples for N47 E011 tile

**Why second:** drop-in compatible — same BIL format, same coordinate system, just more samples.
Zero code changes required; purely a data swap. 9× more pixels = first real performance stress test.

**Format:** ENVI BIL with `.hdr` sidecar — identical to current setup.

**Download:**
- earthexplorer.usgs.gov → register (free) → Digital Elevation → SRTM 1 Arc-Second Global
- Search for tile N47E011, download as BIL

**What needs to change:** nothing. Update `tile_path` in `main.rs` and run.

**Memory math:**
- 10800 × 10800 × 2 bytes = **~234 MB** heightmap (vs ~26 MB current)
- GPU texture R16Float: same 234 MB on GPU — fine on M4 unified (400 GB/s), fine on GTX 1650 (4 GB VRAM)
- Shadow computation: O(rows × cols) DDA sweeps — 9× more work → startup time increases significantly

**f16 precision note:** `R16Float` texture stores heights as half-precision. f16 represents integers
exactly up to 2048; above that it rounds to multiples of 2. Alps heights reach 3800m → 1m rounding
above 2048m. Visually negligible, but worth knowing. (Current 30m data has the same rounding.)

**Expected outcome:** noticeably sharper ridgelines, individual peaks resolved. Shadow computation
startup takes measurably longer — good opportunity to time it and reason about O(N²) scaling.

---

### Part 0.3 — Copernicus GLO-10 (~10m, cleaner than SRTM 1/3)

**Resolution:** ~10m — same pixel density as SRTM 1/3 but with Copernicus quality (better voids,
cleaner artefacts). Coverage for N47 E011 may be partial — check availability first.

**Format:** GeoTIFF (same as GLO-30) — reuses whatever reader was built in Part 0.1.

**Download:**
- AWS: `s3://copernicus-dem-10m/` (where available)
- OpenTopography: opentopography.org → Global Datasets → Copernicus GLO-10

**What needs to change:** same as Part 0.1 (GDAL convert or GeoTIFF reader). If Part 0.1 reader
was generalised, this is a pure data swap.

**Expected outcome:** same resolution as SRTM 1/3 but cleaner surface — good A/B comparison
to see how much SRTM radar artefacts affect the rendered image.

---

### Part 0.4 — Austria 1m LiDAR

**Resolution:** 1m — 27× finer than SRTM 30m. Individual trees, buildings, rock faces visible.

**Why last:** significant engineering work — different projection, tiled delivery, large data volume.

**Download:**
- data.bev.gv.at (Bundesamt für Eich- und Vermessungswesen) — free, requires registration
- Format: GeoTIFF or XYZ point cloud, MGI Transverse Mercator projection (EPSG:31254–31256)
- Delivered as small sheets (~5 km × 5 km each, ~5000×5000 samples per sheet)

**Critical differences from SRTM:**

1. **Projection:** MGI Transverse Mercator, not WGS84 geographic. Coordinates are in **metres**,
   not degrees. The `dx_meters = dx_deg * 111320 * cos(lat)` formula in `parse_bil` is wrong —
   `dx_deg` is already in metres; no conversion needed. The `origin_lat`/`origin_lon` fields would
   store projected easting/northing, not lat/lon.

2. **Tiling:** a 1°×1° cell at 1m resolution = ~100,000×100,000 = 10 billion samples = ~20 GB.
   Nobody ships it as one tile. You'd receive ~400 sheets of ~5 km × 5 km. To cover the same area
   as the current SRTM tile you'd need to stitch ~400 sheets into a single in-memory grid — or
   implement out-of-core streaming (Part 0.4 is a natural prerequisite for Part 5 tile streaming).

3. **Data type:** heights as `f32` (sub-metre precision from LiDAR returns) rather than `i16`.
   The `Heightmap.data: Vec<i16>` field would need to become `Vec<f32>`, or a parallel struct
   created. The GPU R16Float texture has ~3 decimal digits of precision — sufficient at 1m resolution
   (heights in range 500–4000m, error < 0.5m).

**What needs to change in the codebase:**
- Sheet stitching: load N sheets, assemble into one flat array with correct row/col offsets
- Coordinate handling: pass projected coordinates directly as metres, bypass degree→metre conversion
- Possibly: new `parse_geotiff_f32()` function if data is `f32`
- GPU upload: if switching `data` to `Vec<f32>`, the R16Float texture upload path in `scene.rs` changes

**Expected outcome:** visually stunning — individual terrain features at 1m. Shadow precision limited
by the 5° soft-shadow penumbra parameter. Ray step size would need to drop to ~1m (currently ~20m)
which will dramatically increase ray march cost per pixel — good experiment.

---

### Part 0 — Compatibility Summary

| Source | Format | Projection | Compatible? | Work required |
|---|---|---|---|---|
| Copernicus GLO-30 | GeoTIFF | WGS84 geographic | Needs converter | GDAL one-liner or small reader |
| SRTM 1/3 arc-sec | ENVI BIL | WGS84 geographic | **Drop-in** | Just update `tile_path` |
| Copernicus GLO-10 | GeoTIFF | WGS84 geographic | Needs converter | Reuse GLO-30 reader |
| Austria LiDAR 1m | GeoTIFF / XYZ | MGI Transverse Mercator | No | New reader + projection + stitching |

---

## 1 — Shadow toggle (`.` key)

**What:** press `.` to enable/disable sun shadows. When off, terrain is fully lit with no shadow
darkening — useful for comparing shadow quality and for debugging.

**HUD display:** new line in the settings HUD panel (below AO mode):
```
Shadows: On    (Press . to toggle)
Shadows: Off   (Press . to toggle)
```

**Implementation sketch:**
- `shadows_enabled: bool` field on `Viewer`, default `true`
- `.` key flips the bool
- `shadows_enabled: u32` added to `CameraUniforms` (replace one `_pad` field — no size change)
- Shader: `shadow_factor = select(1.0, 0.5 + 0.5 * in_shadow, cam.shadows_enabled == 1u)`
- Shadow background thread continues running — toggle only affects shading, not computation

---

## 2 — Fog toggle (`,` key)

**What:** press `,` to enable/disable atmospheric fog. When off, the full terrain is visible
to the ray's max distance with no haze blend.

**HUD display:** new line in the settings HUD panel (below shadow toggle):
```
Fog: On    (Press , to toggle)
Fog: Off   (Press , to toggle)
```

**Implementation sketch:**
- `fog_enabled: bool` field on `Viewer`, default `true`
- `,` key flips the bool
- `fog_enabled: u32` added to `CameraUniforms` (replace one `_pad` field — no size change)
- Shader: `fog_blend = select(0.0, fog_t, cam.fog_enabled == 1u)` then use `fog_blend` in the
  three `mix` calls

---

## 3 — Visual Artifact Tolerance (`;` key, 4 modes)

**What:** cycle through 4 quality/performance presets that trade rendering accuracy for fps.
Controls two coupled parameters: base step size and the distance-based LOD step multiplier.

**Modes:**

| Mode | `step_m` | Sphere factor | Effect |
|---|---|---|---|
| Ultra | `dx / 20` | `0.1` | No arc artifacts, no ridge misses |
| High  | `dx / 10` | `0.2` | Current default |
| Mid   | `dx / 5`  | `0.4` | Slight arc artifacts near camera |
| Low   | `dx / 3`  | `0.8` | Visible artifacts, fastest |

**HUD display:** new line in the settings HUD panel:
```
Quality: Ultra    (Press ; to change)
Quality: High     (Press ; to change)
Quality: Mid      (Press ; to change)
Quality: Low      (Press ; to change)
```

**Implementation sketch:**
- `vat_mode: u32` on `Viewer` (0=Ultra, 1=High, 2=Mid, 3=Low), default `1`
- `;` key increments via `rem_euclid(4)`
- `step_m` in the `build_camera_uniforms` call computed from `vat_mode`:
  `dx / [20, 10, 5, 3][vat_mode]`
- `vat_mode: u32` added to `CameraUniforms` (replace one `_pad` field)
- Shader: branch on `cam.vat_mode` to pick `sphere_factor` and `base_mult`:
  `t += max((pos.z - h) * sphere_factor, cam.step_m * base_mult + ...)`
  (LOD distance divisors come from `lod_mode` — see item 4)

---

## 4 — LOD Distance (`'` key, 4 modes)

**What:** controls how aggressively LOD kicks in with distance. Two parameters move together:
the step-size growth divisor (`shader_texture.wgsl` line 220) and the mipmap LOD divisor
(`shader_texture.wgsl` line 203). Larger divisor = LOD starts further away = better quality,
less fps gain.

**Modes:**

| Mode  | Step LOD divisor | Mip LOD divisor | LOD=1 starts at |
|---|---|---|---|
| Ultra | ∞ (no growth)  | ∞ (always mip 0) | never |
| High  | `20000`        | `30000`          | 30 km |
| Mid   | `8000`         | `15000`          | 15 km (current) |
| Low   | `4000`         | `8000`           | 8 km |

**HUD display:**
```
LOD: Ultra
LOD: High
LOD: Mid
LOD: Low
```

**Implementation sketch:**
- `lod_mode: u32` on `Viewer` (0=Off, 1=Subtle, 2=Normal, 3=Aggressive), default `2`
- `'` key increments via `rem_euclid(4)`
- `lod_mode: u32` added to `CameraUniforms` (replace one `_pad` field)
- Shader: branch on `cam.lod_mode` to pick `step_div` and `mip_div`:
  `let lod_min_step = cam.step_m * (base_mult + t / step_div)`
  `let mip_lod = log2(1.0 + t / mip_div)` (Off mode: `mip_lod = 0.0`, skip formula)

---

## Notes

- Both toggles reuse the existing `CameraUniforms` `_pad` fields — no bind group or buffer size changes.
- Settings HUD panel accumulates: AO mode label already there; shadow and fog labels stack below it.
- No new textures, buffers, or pipelines needed for either feature.

---

## ~~5 — Out-of-core tile streaming~~

~~**What:** support terrain datasets larger than GPU VRAM by keeping only the tiles
visible from the current camera position resident on the GPU.~~

~~**Why it's interesting at the hardware level:**~~
~~- Exposes the CPU→GPU upload bandwidth as a distinct bottleneck (separate from shader compute)~~
~~- On M4 unified memory, "upload" is a pointer hand-off — near zero cost~~
~~- On discrete GPU (PCIe), a 26 MB tile upload ≈ 1.7ms; 4 new tiles/frame = 6.8ms stall~~
~~- The measurable question: at what camera speed does streaming become the bottleneck?~~

~~**Key concepts to build:**~~
~~1. **Spatial tiling on disk** — world divided into fixed-size chunks (e.g. 256×256 height~~
~~   samples), each a seekable region. Camera position maps to a set of visible chunk coords.~~
~~2. **GPU tile cache** — fixed-size 2D texture array on the GPU (e.g. 8×8 = 64 slots).~~
~~   LRU eviction when a new tile is needed and all slots are full.~~
~~3. **Indirection table (page table)** — small texture the shader consults:~~
~~   `tile_id → slot_index`. If tile not resident, render a lower-res fallback or flag the pixel.~~

~~**This is what GPU hardware calls:** sparse/virtual textures ("tiled resources" in DirectX,~~
~~"sparse binding" in Vulkan). Building it manually makes the abstraction concrete.~~

~~**Prerequisite:** multi-tile SRTM data (currently only one tile loaded).~~

**Superseded by:** `docs/planning/multi-tile-multiple-resolution-load.md`
