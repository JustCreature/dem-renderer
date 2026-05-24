# Water-mask raster for pixel-perfect coastlines

## Background

Today the renderer guesses "is this pixel water?" from the height value alone. The
shader uses a two-step blend around the waterline (sea → turquoise → land, currently
tuned to `[0, 15.5]` and `[15.5, 17]` for NOAA Oahu LiDAR), and the value `15.5 m`
is hand-tuned per dataset because there is no single elevation that cleanly separates
"ocean" from "land" — even after `clamp_nodata_to_sea` flattens the `-9999` sentinels.

`docs/improvements/nodata_policy.md` covers why the elevation alone is ambiguous. The
short version: coastal LiDAR DEMs contain real non-zero values over water (sea-surface
returns at tide stage, vertical-datum offsets, bathymetry in shallow areas, surf-zone
triangulation, mixed-class pixels along the coast). No height threshold can cleanly
separate them from genuine low-elevation land.

The *real* "is this pixel water?" signal lives outside the elevation grid — in a
separate **water-mask raster** that many high-quality coastal DEM products ship
alongside the elevation.

## What a water-mask raster is

A water mask is a co-registered, per-pixel raster that flags water cells. Common forms:

- **Binary** (`uint8`, 0/1 or 0/255): "this pixel is water". One bit per pixel.
- **Class index** (`uint8`): a NLCD-style classification — 0 land, 1 inland water,
  2 ocean, 3 tidal flat, etc. Lets the renderer distinguish lakes from sea.
- **Continuous** (`float32` in [0, 1]): probability or fractional-water-coverage of
  each cell. Most useful for sub-pixel coastline reconstruction.

The mask is *always* in the same CRS, projection, origin, and pixel grid as the
companion DEM — by construction, since it's produced from the same source point
cloud and the same triangulation.

## Where masks come from

| Source | Mask product |
|---|---|
| **NOAA coastal LiDAR** (incl. Oahu 2013) | Ships a `*_water_mask.tif` or similar in the data bundle. |
| **USGS 3DEP / NED** | Provides `NLCD_*` land-cover rasters at matching resolution; class 11 = open water. |
| **Copernicus DSM (GLO-30 / GLO-90)** | Ships `WBM` (Water Body Mask) auxiliary product. Same grid as the elevation tile. |
| **NZ LINZ LiDAR** | Per-tile water polygon vectors in the metadata bundle (rasterise on load). |
| **SRTM** | No first-party mask. Best fallback is OpenStreetMap coastlines or the GSHHG dataset. |

For sources without an explicit mask, two derivable signals exist:

1. **Surface roughness from the normal map.** Water surfaces in a gridded DEM are
   spectacularly smooth (variance < 0.1 m across many pixels). Coastal land is
   never that flat. A "low elevation *and* low normal variance" mask gets close to
   the right answer for free.
2. **OpenStreetMap coastline polygons.** Rasterise the relevant `coastline` ways
   onto the DEM grid at load time. Network-free if the OSM extract is bundled.

## What the renderer would do with a mask

Three concrete uses, in increasing order of payoff:

### 1. Replace the height-based coastal palette

Instead of `mix(land, sea, smoothstep(0, 2, pos.z))` and the turquoise band, the
shader gets a `water_mask: texture_2d<f32>` (R8Unorm) bound alongside `hm_tex`,
and computes:

```wgsl
let water_w = textureSampleLevel(water_tex, water_samp, uv, 0.0).r;
let coastal = mix(sea, turquoise, water_w);  // or skip turquoise entirely
let base = mix(land, coastal, water_w);
```

Pixel-perfect coastline; no hand-tuned thresholds; no false-positive lake renders;
no missing shallow-reef colour. Works for any dataset that ships a mask.

### 2. Decouple shadow / AO computation from water cells

The bigger win is on the CPU side. Water-cell normals are meaningless (a glassy
flat surface gives `(0, 0, 1)` uniformly), but they currently feed `compute_normals_*`
and contribute to shadow DDA and AO sweep — producing spurious flat-shaded patches
and zero-AO holes adjacent to the coast. With a mask, those passes can skip water
cells (or assign them `nz = 1`, AO = 1, shadow = 0) without polluting the actual
terrain computations.

### 3. Render real water (animated, reflective)

Once the mask exists, water cells could get a separate render path: animated
shader-only surface (Gerstner waves, sky reflection sampled via the existing
fog/sky colour, view-direction-dependent shading). This is a separate effort — far
beyond a docs/improvements doc — but the mask is the *enabling* dependency.

## Implementation sketch

The minimum viable shape:

- **`dem_io::WaterMask`** — a sibling of `Heightmap`, same `cols`/`rows`/CRS/origin,
  but `data: Vec<u8>` (0 = land, 255 = water). Loaded by `parse_water_mask_auto(path)`
  with the same CRS-detection logic as `parse_geotiff_auto`.
- **Source pairing convention** — the launcher's per-tier path becomes
  `(elevation: PathBuf, water_mask: Option<PathBuf>)`. `LauncherSettings::demo_view`
  gains an optional sibling field. For sources that auto-discover a mask (NOAA's
  `*_water_mask.tif` next to `*.tif`), the loader can fill it in implicitly.
- **GPU binding** — one new R8Unorm texture + sampler in the canonical bind group,
  uploaded by the same tier worker that uploads the heightmap. Cost: ~`cols × rows × 1`
  byte per tier. For a `8192 × 8192` base tier that's 64 MB; for typical 5 m / 1 m
  windows a few MB.
- **Shader uniform flag** — `water_mask_present: u32` in `CameraUniforms`. When
  zero, the shader falls back to the current height-based path. Keeps datasets
  without a mask working unchanged.

## What blocks this from happening today

- **CRS-aware co-registration.** A mask raster with the same dimensions as the
  elevation is easy. A mask raster from a *different* source (e.g. OSM-derived
  coastline rasterised at our grid) requires reprojecting + resampling at load
  time. We already have `proj4rs` plumbing; the warp pass is the new work.
- **Source-discovery convention.** The launcher needs a path-or-policy field on
  every tier so the user can either explicitly attach a mask or let the loader
  guess. Tied into the future `NodataPolicy` enum (`docs/improvements/nodata_policy.md`)
  — both are per-source metadata that ideally belong to the same struct.
- **VRAM accounting.** `vram.rs::create_texture_tracked` already counts every
  texture; a new R8Unorm bound per tier just adds entries. Worth checking that the
  Low VRAM preset still fits after the extra ~3–5 MB per tier.

## When to do it

Trigger: the moment we ship a built-in preset that uses NOAA topobathy or any
LiDAR product where the height-only heuristic produces obviously wrong coastlines.
Diamond Head is the first such case — today's `15.5 m` tuning is a workaround.
If we add a second NOAA tile with a different tide-stage offset, the workaround
breaks and the mask becomes mandatory.

Until then the height-based palette is acceptable for SRTM and Copernicus tiles
(where every pixel is genuinely land or genuinely outside the tile) and tolerable
for hand-tuned single-source coastal extracts.

## Out of scope

- Multi-class land cover (vegetation, urban, snow zones) — the same plumbing would
  support it, but that's a colour-palette redesign, not a water-vs-land question.
- Animated water surface — depends on the mask but is its own effort.
- Bathymetric rendering (showing sea-floor depth as gradient blue) — possible
  with the same mask + the raw bathymetric values from the source, but again a
  separate visualisation choice.
