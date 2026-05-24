# NoData policy

## Background

Every DEM has cells where it has no information. The renderer needs *some* value for
those cells — the question is which value, and who decides.

`dem_io::extract_window` writes a single sentinel — `-9999.0` — into any output cell
that the source TIFF reports as nodata, never read (out-of-strip), or NaN. That
sentinel is unambiguous, but it is *not* a height. If the renderer treats it as a
height, a coastal extract turns into a 9 km chasm where the ocean should be.

This document records the interim policy we ship today, the failure modes it does and
doesn't cover, and the per-source policy enum we expect to add later.

## What "no data" actually means

`-9999` is semantically overloaded. In real datasets the same sentinel covers three
unrelated cases:

| Case | Example | Right value |
|---|---|---|
| **Ocean / known void** | NOAA Oahu LiDAR over Pacific south of Diamond Head | 0 m (sea level) |
| **Acquisition gap** | SRTM voids in cloud-shadowed valleys | guess from neighbours (`fill_nodata`) |
| **Out-of-tile** | extract_window window extending past tile edge | depends on adjacent tile / shader bounds check |

Distinguishing them requires knowledge that lives outside the pixel — the source
catalogue or the user's intent. Today the renderer has neither; it just sees `-9999`.

## Interim policy (this commit)

**Single rule applied everywhere: replace `-9999` (and anything below `-1000`) with
`0.0` m sea level.** Implemented at two layers for defence in depth:

1. **CPU-side at upload boundary** — `dem_io::clamp_nodata_to_sea(&mut Heightmap)`
   walks the data buffer once and overwrites any `h < -1000.0` with `0.0`. Called
   from every tier worker after `extract_window` + `fill_nodata_from_base` and from
   the initial scene-init paths in `src/viewer/scene_init.rs`. The clamp runs *after*
   `fill_nodata_from_base`, so any cell the base-spread fill could resolve still gets
   resolved, and only the residual coastal/void cells are flattened.

2. **GPU-side fallback in shader** — every base-tier `textureSampleLevel(hm_tex, …)`
   site in `crates/render_gpu/src/shader_texture.wgsl` is wrapped with
   `max(…, SEA_LEVEL_M)`. Close and fine tiers already gate their blends with
   `if h5 > -1000.0`, which keeps the base value visible where the overlay has no
   data — that path is unchanged. The shader fallback exists so that a bug in any
   future loader (or a path we forgot to wire) can never render as a chasm again; the
   worst-case visual is "flat sea where there should be terrain", not "infinite hole".

Both layers are intentional. CPU clamp keeps derived data clean — normals, shadow,
and AO all see `0 m` instead of `-9999 m`, so we don't compute a `+9999` slope at the
coast or a giant shadow projecting from a phantom cliff. Shader fallback is the
safety net that doesn't depend on remembering to call the CPU helper.

## What this is wrong for

This policy is *correct* for any tile whose nodata is genuinely ocean (every coastal
LiDAR product) and *acceptable* for any tile whose nodata is small enough that being
filled with neighbours wouldn't make a meaningful difference. It is *wrong* for:

- **Inland nodata gaps** in datasets that aren't otherwise oceanic. A mid-valley
  SRTM void will now render as a flat 0 m disc instead of a smooth interpolation
  from the neighbouring slopes. For tiles centred on terrain that *should* be at
  altitude, the disc will look obviously wrong.
- **Datasets whose origin is below sea level**, e.g. Dead Sea (-430 m). Real terrain
  there is below `SEA_LEVEL_M`; our fallback would clip it. The `-1000` threshold
  was chosen to leave headroom for known sub-sea-level basins, but if we ever load
  such tiles we should reconsider.
- **Bathymetry sources** that report negative heights as sea-floor depth. Our policy
  treats those as sentinels and zeroes them. Bathymetry is out of scope today, but
  worth keeping in mind.

## Future: per-source NodataPolicy enum

The right long-term shape is to make the policy explicit and per-source. Sketch:

```rust
pub enum NodataPolicy {
    /// Replace cells where h < threshold with 0.0 m. Today's default. Best for
    /// coastal LiDAR, single-source UTM/marine tiles.
    ClampToSea { threshold: f32 },

    /// Spread nearest valid neighbour into nodata cells. Best for SRTM-style gap fill
    /// where the gap is small relative to surrounding terrain. O(N²) or worse — only
    /// runs once at load time.
    FillNeighbours,

    /// Leave the -9999 sentinel in place. The shader/consumer is expected to detect
    /// it explicitly. Lets a future renderer skip nodata cells (transparent sky,
    /// fog, alt overlay) instead of faking a value.
    LeaveSentinel,
}
```

Where it lives:

- One policy per *source*, attached to the `TileEntry` (or to the loader call). The
  demo view config and the per-tier paths in `LauncherSettings::demo_view` should
  carry it. CLI / launcher UI exposes a per-source dropdown.
- Applied at the boundary between `dem_io` and the renderer — i.e. exactly where
  `clamp_nodata_to_sea` lives today. The function becomes
  `apply_nodata_policy(&mut Heightmap, &NodataPolicy)`.
- The shader fallback stays *unconditionally on* even when the CPU policy is
  `LeaveSentinel`, because `LeaveSentinel`'s contract is "consumer handles it" — the
  shader is one of those consumers.

### What forces the enum to land

Two situations bring it from "future work" to "now":

1. A real-world SRTM-style void shows up in a dataset we want to ship as a preset,
   and the flat-sea disc is visually objectionable. `FillNeighbours` becomes the
   right per-source policy.
2. A user requests a bathymetric or below-sea-level dataset. The `ClampToSea`
   threshold becomes wrong for that source; per-source policy lets each source
   carry its own threshold or its own mode.

Until one of those lands, the single global `ClampToSea` rule is the right tradeoff
between code surface and correctness.

## Notes / open follow-ups

- `fill_nodata` in `dem_io::heightmap` still has the O(N³) worst case + div-by-zero
  edge case flagged in `docs/improvements/algo_fill_nodata_improvement.md`. Those
  need to land before `FillNeighbours` becomes a real shipping policy.
- The `-1000.0` threshold is chosen in two places: `dem_io::clamp_nodata_to_sea` and
  the close/fine tier blend gates in `shader_texture.wgsl`. If we introduce a
  per-source threshold the constant should move into `CameraUniforms`.
- `SEA_LEVEL_M = 0.0` in the shader is the absolute sea level in metres. If we ever
  let the user view tiles whose vertical CRS is not orthometric (rare), this
  becomes a per-source datum offset rather than a global constant.
