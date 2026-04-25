# Phase 8 — Higher-Resolution Data, Viewer Controls, and Multi-Resolution Streaming Architecture

**Hardware**: Apple M4 Max (primary)
**Dataset**: Copernicus GLO-30 (3601×3601, ~30m), BEV DGM 5m (EPSG:31287), BEV LiDAR 1m (EPSG:3035, 8001×8001)
**Date**: 2026-04-20 to 2026-04-25

---

## Overview

Phase 8 had three distinct streams of work. The first — and most infrastructure-heavy — was
integrating progressively finer DEM data sources, each using a different map projection. This
forced a deep engagement with coordinate reference systems (CRS), GeoTIFF internals, and the
mismatch between geographic and projected coordinate spaces. The second was wiring four runtime
viewer controls (shadow, fog, quality, LOD) cleanly through the GPU uniform pipeline without
any new bindings or buffer resizes. The third was designing — but not yet implementing — a
multi-tier multi-resolution streaming architecture, culminating in a geometric argument for why
a 3×3 tile grid is necessary at 47°N and a full five-step plan for Phase 9.

---

## Part 1 — Map Projections and GeoTIFF Diversity

### 1.1 What a CRS is and why it matters

A coordinate reference system defines how a pair of numbers (coordinates) maps to a location
on Earth's curved surface. Two files can cover the same mountain and yet have completely
incompatible coordinates if they use different CRSes. Understanding this is the entry point to
working with any real-world geodata.

Earth is not a sphere. It is an oblate spheroid — flattened at the poles, wider at the equator.
Different geodetic datums define slightly different ellipsoids to approximate it. The two
datums that appear in Phase 8 are:

- **GRS 1980** (Geodetic Reference System 1980): the ellipsoid underlying WGS84 (GPS),
  ETRS89, and modern European standards. Semi-major axis a = 6,378,137 m,
  flattening f ≈ 1/298.257.
- **Bessel 1841**: an older ellipsoid fitted to Europe before satellite geodesy. Semi-major
  axis a = 6,377,397 m — about 740 m smaller at the equator. Still used by BEV (Austrian
  survey authority) for historical DGM 5m data.

The difference matters: coordinates in EPSG:31287 (Bessel 1841) cannot be directly compared to
coordinates in EPSG:3035 (GRS 1980). A point at (273605, 356962) in EPSG:31287 and
(4449262, 2663978) in EPSG:3035 refer to the same physical location — Hintertux glacier — but
the numbers are completely different.

### 1.2 EPSG:4326 — WGS84 Geographic (GLO-30)

This is the familiar latitude/longitude system. Coordinates are in decimal degrees. A 1°×1°
cell covers roughly 111 km × (111 km × cos(lat)) — narrowing east-west as you move away from
the equator.

Copernicus GLO-30 delivers one GeoTIFF per 1°×1° degree cell. The existing pipeline already
handled this format: `dx_meters = dx_deg * 111320 * cos(lat_rad)`.

The GeoTIFF metadata tags that carry coordinate information:
- `ModelPixelScaleTag` — (Δx, Δy, Δz) per pixel in CRS units. For geographic data, units are
  degrees, so Δx ≈ 0.000278° (≈ 30m at 47°N).
- `ModelTiepointTag` — (pixel_x, pixel_y, 0, crs_x, crs_y, 0) giving one anchor point.
  Together with pixel scale, this gives the full affine transform: any pixel (col, row) maps
  to `(crs_x + col * Δx, crs_y - row * Δy)` (note: rows increase downward, northing increases
  upward, hence the sign flip).

**Pixel scale as a CRS discriminator**: geographic data has Δx < 0.1 (degrees per pixel);
projected data has Δx ≥ 1.0 (metres per pixel). Reading `ModelPixelScaleTag[0]` and checking
this threshold is enough to identify the CRS family without parsing the full GeoTIFF
projection metadata.

### 1.3 EPSG:31287 — Austria Lambert (BEV DGM 5m)

Lambert Conformal Conic (LCC) is a projection onto a cone intersecting Earth at two standard
parallels. Meridians become straight lines radiating from the cone's apex; parallels become arcs
of circles. Scale is exact only at the standard parallels and increases with distance from them.

EPSG:31287 parameters:
- **Datum**: MGI (Militärgeographisches Institut), Bessel 1841 ellipsoid
- **Standard parallels**: φ₁ = 46°N, φ₂ = 49°N
- **Central meridian**: λ₀ = 13° 20' E (13.333°E)
- **Latitude of origin**: φ₀ = 47° 30' N (47.5°N)
- **False easting / northing**: 400,000 m / 400,000 m

The LCC forward projection maps (lat, lon) → (easting, northing) via:

```
n  = ln(cos φ₁ · sec φ₂) / ln(tan(π/4 + φ₂/2) · cot(π/4 + φ₁/2))
F  = cos φ₁ · tan^n(π/4 + φ₁/2) / n
ρ  = a · F · cot^n(π/4 + φ/2)          (where a = Bessel semi-major axis)
ρ₀ = a · F · cot^n(π/4 + φ₀/2)
θ  = n · (λ − λ₀)
E  = FE + ρ · sin θ
N  = FN + ρ₀ − ρ · cos θ
```

BEV DGM 5m delivers the whole of Austria as a single GeoTIFF (~600 km × 300 km =
~120,000 × 60,000 pixels). You cannot load it whole. The immediate workaround was
`gdal_translate -projwin` to extract a window; the long-term solution (Phase 9 Step 2) is
in-process windowed reading.

**NoData sentinel**: BEV DGM 5m uses `0.0` (not NaN, not -9999). This is safe because every
valid Austrian elevation is well above 0 m — but it means you cannot skip the NoData check
even for seemingly clean data.

### 1.4 EPSG:3035 — LAEA Europe (BEV LiDAR 1m)

Lambert Azimuthal Equal Area (LAEA) projects the globe onto a plane tangent at one point.
It is *equal-area*: every region of equal area on Earth occupies equal area on the map,
regardless of shape distortion. This makes it useful for continent-scale analysis.

EPSG:3035 parameters:
- **Datum**: ETRS89, GRS 1980 ellipsoid
- **Projection centre**: φ₀ = 52°N, λ₀ = 10°E
- **False easting / northing**: 4,321,000 m / 3,210,000 m

The forward projection (spherical approximation, accurate to ~100 m for regional data):

```
k  = sqrt(2 / (1 + sin φ₀ sin φ + cos φ₀ cos φ cos(λ − λ₀)))
E  = FE + R · k · cos φ · sin(λ − λ₀)
N  = FN + R · k · (cos φ₀ sin φ − sin φ₀ cos φ cos(λ − λ₀))
```

where R = 6,371,000 m (mean spherical radius). The inverse recovers (lat, lon) from (E, N).

BEV delivers 1m LiDAR data as 50 km × 50 km sheets (50,000 × 50,000 pixels = ~10 GB each).
For Phase 8 we extracted 8 km patches using `gdal_translate`. The coordinate scheme encodes
the sheet origin in the filename: `CRS3035RES50000mN{northing}E{easting}.tif` — this naming
convention becomes the source-tile registry in Phase 9 Step 4.

**Key constraint discovered**: `ModelPixelScaleTag[0]` = 1.0 for 1m data (1 metre per pixel).
The pixel-scale discriminator (`< 0.1` = geographic, `>= 1.0` = projected) correctly
identifies this as EPSG:3035 (not 31287) when combined with the scale magnitude: `scale >= 5.0`
→ 31287, `scale >= 1.0` → 3035, else geographic.

### 1.5 wgpu Resource Limits

Two hard limits were hit with 8001×8001 tiles:

**Texture dimension limit (8192 px)**: wgpu's `Limits::default()` caps 2D texture dimensions
at 8192 per axis. This is a hardware minimum guarantee — all modern GPUs support at least
8192 × 8192. The M4 Max supports 16,384 × 16,384. Requesting `adapter.limits()` in
`DeviceDescriptor::required_limits` unlocks the hardware maximum, but the 8192 limit must be
kept in mind when designing the 3×3 assembled grid (see Part 3).

**Max storage buffer binding size (128 MB)**: wgpu default is 128 MB. For an 8001 × 8001 ×
4-byte heightmap, the storage buffer would be ~256 MB. Same fix: `adapter.limits()`.

**`tiff` crate memory limit**: the Rust `tiff` crate imposes its own ~128 MB guard on decoded
image size. Fix: `Decoder::with_limits(Limits::unlimited())`. This is independent of wgpu.

### 1.6 Camera Positioning Across CRSes

The viewer stores one named camera position as WGS84 lat/lon (47.076211°N, 11.687592°E).
At startup, `latlon_to_tile_metres` dispatches on `hm.crs_epsg`:
- EPSG:4326: direct degree-to-pixel mapping via the affine transform
- EPSG:31287: `lcc_epsg31287(lat, lon)` forward projection → tile-local metres
- EPSG:3035: `laea_epsg3035(lat, lon)` forward projection → tile-local metres

If the named position falls outside the tile's bounding box, the function returns `None` and
the viewer falls back to a hardcoded pixel offset. This graceful fallback is important when
switching between tiles that may or may not contain the named point.

**The lesson**: store camera positions in a CRS-independent format (WGS84). Forward-project at
runtime. Never hardcode pixel coordinates into viewer state.

---

## Part 2 — Runtime Viewer Controls: Extending CameraUniforms

### 2.1 The Uniform Buffer Layout Problem

`CameraUniforms` is a Rust struct marked `#[repr(C)]` that is uploaded to the GPU via
`write_buffer`. The WGSL `CameraUniforms` struct in the shader must have exactly the same
memory layout — byte for byte. Any mismatch causes silent misreads: the GPU reads the right
bytes, but interprets them as the wrong field.

WGSL structs follow **std140 layout rules**: scalars align to 4 bytes, vec2 to 8, vec3/vec4
to 16. Each `u32` or `f32` field is 4 bytes. The struct must be padded to a multiple of the
largest member alignment (16 bytes for vec4-containing structs).

The existing `CameraUniforms` had reserved `_pad5` through `_pad9` — five 32-bit slots — after
`ao_mode`. Four of those were replaced with:
- `shadows_enabled: u32`
- `fog_enabled: u32`
- `vat_mode: u32`
- `lod_mode: u32`

`_pad5` was kept to maintain 16-byte group alignment after `ao_mode`. No bind group changes,
no buffer resize, no pipeline rebuild — this is the payoff of reserving padding upfront.

### 2.2 WGSL `select()` — Branchless Enum Dispatch

WGSL has no `if/else` expression (only statements). For per-pixel conditional values in a
shader, the correct tool is `select(false_val, true_val, condition)` — the WGSL equivalent
of a ternary that compiles to a `cmov` or predicated move, not a branch.

For 4-way enum dispatch (e.g. `lod_mode` with values 0–3), chain `select()` calls:

```wgsl
let step_div = select(
    select(
        select(1000000000.0, 4000.0, cam.lod_mode == 3u),
        8000.0,
        cam.lod_mode == 2u
    ),
    20000.0,
    cam.lod_mode == 1u
);
```

This evaluates all four constants and selects without branching. The GPU never diverges —
every thread in a warp/simdgroup executes the same path. For uniform values (all pixels use
the same `lod_mode`) a branch would also work, but `select()` is cleaner and avoids the
question of whether the compiler will hoist the branch out of the inner loop.

### 2.3 The Four Controls

**Shadow toggle (`.` key)**:
```wgsl
let shadow_factor = select(1.0, 0.5 + 0.5 * in_shadow, cam.shadows_enabled == 1u);
```
When `shadows_enabled = 0`, every pixel gets `shadow_factor = 1.0` (fully lit). The shadow
DDA sweep continues running on the CPU — toggling only affects shading, not computation.
This means shadow quality can be evaluated A/B in real time without restart cost.

**Fog toggle (`,` key)**:
```wgsl
let fog_blend = select(0.0, fog_t, cam.fog_enabled == 1u);
```
`fog_t` is the smoothstep fog weight (0 at 15 km, 1 at 60 km). When disabled, `fog_blend = 0`
and the three `mix(color, sky_color, fog_blend)` calls in the shader become no-ops.

**VAT quality presets (`;` key)** — Visual Artifact Tolerance:
Controls `step_m` (ray march step size) on the Rust side, and `sphere_factor` (the sphere
tracing aggressiveness multiplier) on the shader side. Both must move together — a smaller
`step_m` with a large `sphere_factor` is incoherent and causes missed intersections.

| Mode  | `step_m = dx/N` | `sphere_factor` | Effect |
|-------|-----------------|-----------------|--------|
| Ultra | dx/20 ≈ 1.0 m  | 0.1             | No artifacts, slowest |
| High  | dx/10 ≈ 2.1 m  | 0.2             | Default quality |
| Mid   | dx/5 ≈ 4.1 m   | 0.4             | Slight arc artifacts near camera |
| Low   | dx/3 ≈ 6.9 m   | 0.8             | Visible artifacts, fastest |

**LOD distance presets (`'` key)**:
Controls two divisors in the shader: the step-size growth divisor (how quickly the ray step
grows with distance) and the mipmap LOD divisor (how quickly higher mip levels are sampled
with distance). Both use the same `select()` pattern.

| Mode  | Step div | Mip div | LOD=1 starts at |
|-------|----------|---------|-----------------|
| Ultra | ∞ (off)  | ∞       | never |
| High  | 20000    | 30000   | ~30 km |
| Mid   | 8000     | 15000   | ~15 km (default) |
| Low   | 4000     | 8000    | ~8 km |

### 2.4 HUD Panel Layout: glyphon Sizing

The Phase 7 HUD used a single-line settings label. Phase 8 expanded this to a 5-line panel
(AO + shadows + fog + quality + LOD). Two independent size parameters control the layout:

- **`set_size(width, height)`** on the glyphon `Buffer`: controls the text layout box. If the
  height is smaller than the total line height, text is clipped. Must be ≥ `num_lines * line_height`.
- **Background rectangle vertices** in `build_vertices()`: drawn as a separate quad behind the
  text. Sized and positioned independently — changing `set_size` does not automatically resize
  the background. Both must be updated consistently.

This is a common source of bugs when adding lines to a HUD: the text overflows the layout box
(invisible) or the background rectangle is too small (text visible but unreadable against the
scene).

---

## Part 3 — Multi-Resolution Streaming Architecture

### 3.1 The Problem: Why One Tile Is Not Enough

The current renderer loads one tile at startup and renders it. This works while the camera
stays within the tile's bounds. At the tile edge, the raymarcher hits the boundary and returns
sky — a hard visual cutoff. With fog distance 60 km and SRTM tiles spanning ~76–111 km, a
camera near the tile edge sees a fog-free hard boundary.

The deeper problem: real-world terrain is continuous. An interactive viewer should let the user
fly anywhere without hitting invisible walls.

### 3.2 Why Hardware Sparse Textures Are Not Available

The natural GPU-native solution is **sparse/virtual textures**: a partially-resident texture
where unmapped pages return a defined fallback value. The hardware TLB walks the residency
table per texel fetch, transparently. This is exposed as:
- DirectX 12: Tiled Resources (`D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS` + tile mappings)
- Vulkan: `VK_EXT_sparse_binding` (sparse memory binding for textures)
- Metal: `MTLSparseTexture` (Metal 3+, Apple Silicon)

**wgpu does not expose any of these.** The wgpu abstraction layer explicitly excludes sparse
resource management. Accessing Metal sparse textures from wgpu would require dropping to the
`wgpu::hal` unsafe backend interface — bypassing the cross-platform abstraction entirely.

The software alternative — a texture array + an indirection table — implements exactly the same
conceptual model: logical tile address → physical slot lookup → texture sample. The difference
is where the lookup happens: GPU fixed-function silicon (hardware) vs shader ALU (software).

### 3.3 Three Resolution Tiers

Rather than one resolution level with many tiles, Phase 9 will use three concentric tiers:

| Tier | Source | Resolution | Spatial extent |
|------|--------|------------|----------------|
| 30m  | Copernicus GLO-30 | ~30 m/px | Unlimited — 3×3 sliding grid |
| 5m   | BEV DGM Austria | 5 m/px | Default 10 km radius from camera |
| 1m   | BEV LiDAR 50km sheets | 1 m/px | Default 5 km radius from camera |

The GPU holds three heightmap textures simultaneously. The shader selects the finest resident
tier at each ray position and blends across tier boundaries.

This structure matches the actual data delivery: GLO-30 is per-degree tiles, BEV 5m is a
single national file, BEV 1m is 50 km sheets. There is no point pre-processing these into a
common chunked format — the sources themselves determine the loading granularity.

### 3.4 Tile Geometry at 47°N: Why 3×3

This is one of the key geometric insights of Phase 8.

SRTM/GLO-30 tiles are 1° × 1° in geographic coordinates. At 47°N:
- **N-S dimension**: 1° latitude = **111 km** (constant everywhere on Earth)
- **E-W dimension**: 1° longitude × cos(47°) = 111 × 0.682 = **≈76 km**

Fog distance is 60 km. From the centre of the camera's tile:
- To the east or west edge: 76/2 = 38 km. Fog extends 60 km → **overshoots by 22 km, always**.
- To the north or south edge: 111/2 = 55.5 km. Fog extends 60 km → overshoots by only **4.5 km** from dead centre.

From any position in the southern half of the tile (camera is < 55.5 km from the south edge),
the fog horizon reaches into the next tile south. Symmetric for north. The only safe region
where N/S neighbours are definitely not needed is a narrow 10 km band around the tile's
east-west midline.

**A 2×2 grid is insufficient**: a camera at the NW corner of a 2×2 grid would be missing
tiles to the N, NW, and W. A **3×3 grid** (camera always in the centre tile) covers all
positions correctly.

### 3.5 The Sliding Policy

With 3×3, the natural invariant is: **the camera is always in the centre tile**. Tile identity
is the integer-degree origin (e.g. N47E011). When `floor(camera_lon)` or `floor(camera_lat)`
changes, the centre tile has changed. The grid slides:

1. Drop the 3 tiles on the trailing edge (they are now 2 columns/rows away).
2. Load 3 new tiles on the leading edge.
3. Keep the 6 tiles that are still in the new 3×3 footprint untouched.

This is simpler than a midpoint-crossing trigger: no threshold tuning, no hysteresis needed.
The event is discrete (integer floor changes exactly when a tile boundary is crossed).

### 3.6 The Texture Dimension Problem

Assembling 3×3 tiles naively: 3 × 3601 = 10,803 pixels per axis. This exceeds wgpu's 8192 px
default limit. Even with `adapter.limits()`, the M4 Max supports 16,384 — so it would work on
M4, but not on many other GPUs.

**Solution**: store outer 8 tiles at half resolution (1801 px per tile instead of 3601).
Assembled grid: 1801 + 3601 + 1801 - 2 (seam pixels) = 7,201 px per axis. Well within 8192.

**Why this is geometrically justified**: the outer tiles are only needed beyond ~38 km from
the camera (past the centre tile's E/W edge). At 38 km with 30m resolution data, a pixel
subtends:
```
30 m / 38,000 m = 0.00079 rad ≈ 0.045°
```
This is already smaller than a typical display pixel (a 60° FoV on a 1600-pixel-wide screen
gives 0.0375°/pixel at the screen centre, growing toward edges). Halving the resolution of
the outer tiles (to 60m effective) is invisible at that range — and the fog blend further
attenuates detail beyond ~50 km.

### 3.7 Preprocessing Asymmetry

Full preprocessing (normals + shadows + AO) for all 9 tiles is expensive. But shadows and AO
are only visually significant at close range — within the centre tile.

Strategy:
- **Centre tile**: full normals + shadows + AO (same as today, ~1–2 s for 3601×3601)
- **Outer 8 tiles**: normals only (scalar, single-thread); skip shadows and AO

This reduces preprocessing cost for outer tiles by roughly 3× (shadow sweep dominates at
~1.5 ms/tile for 3601×3601, AO is ~2×), at zero perceptible visual quality loss.

### 3.8 Windowed GeoTIFF Extraction

For the 5m and 1m tiers, the source data is too large to load whole. The required operation is:
given a source GeoTIFF, a centre point in CRS coordinates, and a radius, decode only the rows
and columns within that window.

The `tiff` crate exposes a scanline API: `decoder.read_scanline()` decodes one row at a time.
To extract a window:
1. Compute the pixel row/col of the top-left corner: `row = (crs_y_top - tiepoint_y) / pixel_scale_y`
2. Seek to that row (the `tiff` crate supports this for stripped TIFFs).
3. Decode only the needed rows, slicing each scanline to the column range.

This is what `gdal_translate -projwin` does internally. Implementing it in-process removes the
subprocess overhead and allows integration with the background thread model.

**Multi-source stitching**: when the camera's window crosses the boundary of one BEV source
tile, the adjacent tile must be opened and the complementary strip extracted. The 50 km sheet
naming convention (`CRS3035RES50000mN{Y}E{X}.tif`) gives the adjacent tile's filename from
the current position — no external index needed.

### 3.9 Per-Tier Background Threads

Each resolution tier has one dedicated background thread. The main thread sends camera
position updates over a bounded channel (size 1 — stale positions are dropped). The loader
thread:
1. Computes drift from the current window centre.
2. If `drift > 0.4 * radius`, fires a new windowed extraction.
3. Sends the completed `(Heightmap, normals, shadows, ao)` bundle back over a return channel.

The main thread polls the return channel each frame. If no new tile has arrived (camera moved
faster than the loader), the previous tile for that tier continues rendering. The shader's
distance-based tier selection provides automatic coarser fallback: the camera has moved past
the 40% drift threshold but the 1m tier is still rendering the old window; the 5m and 30m
tiers are still correct and cover the full frame.

**The 40% threshold**: at 40% drift, the camera is 2 km past centre for a 5 km radius tier.
The window edge (5 km from the original centre) is now 3 km ahead. At walking speed (2 m/s)
the loader has ~1,500 seconds — irrelevant. At fly-through speed (100 m/s) it has 30 seconds —
more than enough. At "teleport" speed the loader loses; coarser fallback renders until it catches up.

### 3.10 Multi-Tier Shader Sampling

The shader receives three textures (`hm_30m`, `hm_5m`, `hm_1m`) and two radius uniforms
(`radius_5m_m`, `radius_1m_m`). For each ray hit point it computes the horizontal distance
from the camera, then samples the appropriate tier with a lerp blend zone:

```
dist < radius_1m * 0.9   →  pure 1m sample
dist in [0.9, 1.0] * r1m  →  lerp(1m, 5m)
dist < radius_5m * 0.9   →  pure 5m sample
dist in [0.9, 1.0] * r5m  →  lerp(5m, 30m)
dist > radius_5m * 1.0   →  pure 30m sample
```

The 10% blend zone (~500 m at 5 km radius, ~1 km at 10 km radius) prevents visible seams
where the underlying height data from two sources disagrees slightly at the same real-world
point. The lerp interpolates both height and all derived quantities (normals, shadow) from the
coarser tier using the finer tier's UV mapping — the blend zone thus also smooths any
discontinuities in the normal map at the tier boundary.

**Tier not-yet-loaded signal**: `radius_1m_m = 0.0` means the 1m tier has not yet been
extracted (startup, or loader still running). The shader treats this as "always in 30m/5m
territory" by never satisfying `dist < 0` — correct and safe.

---

## Summary of Key Insights

**GeoTIFF and CRS diversity**
Every real-world DEM dataset uses a different coordinate reference system. Understanding the
datum (which Earth shape), the projection (which mathematical mapping to a flat plane), and
the parameter set is not optional — it is the prerequisite for loading the data correctly.
The pixel scale tag is the fastest CRS discriminator available without parsing full WKT.

**wgpu resource limits need explicit unlocking**
wgpu's default limits are conservative minimums guaranteed across all supported hardware. They
are not the hardware maximum. Always request `adapter.limits()` when working with
production-scale data. The mismatch between "default works in tutorial" and "default fails at
64 MB" is a common source of confusing errors.

**Tile geometry is latitude-dependent**
SRTM/GLO-30 tiles shrink east-west as latitude increases. At 47°N, tiles are 76 km wide but
111 km tall. This asymmetry directly determines the required grid size (3×3 not 2×2) and the
difference in when E/W vs N/S neighbours are needed. Ignoring tile shape leads to subtle
edge-of-tile visual failures that only appear in testing, not development (because you
typically develop with the camera in the tile centre).

**Outer-tile preprocessing asymmetry is justified**
Detail beyond ~38 km at 30m resolution is subpixel. Half-resolution outer tiles save
significant preprocessing time with zero perceptible visual cost, and keep the assembled
texture within the 8192 px GPU limit.

**Hardware sparse textures are unavailable in wgpu; software indirection is equivalent**
The software page table (texture array + indirection lookup) implements the identical
abstraction to hardware sparse textures. The difference is instruction count per texel, not
conceptual correctness. Understanding the software version makes the hardware version
immediately comprehensible when encountered in a lower-level API.

**Uniform struct padding must be maintained on both Rust and WGSL sides**
The GPU reads the raw bytes of the uniform buffer. The WGSL struct declaration must match the
Rust `#[repr(C)]` layout exactly. Reserved `_pad` fields make it safe to add new fields
without changing bind group layouts or buffer sizes — they are consumed one at a time.
