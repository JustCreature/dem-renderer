# Sub-radius dataset IFD selection

## The observation

A 3.6 km × 3.3 km, 1 m/px tile of Diamond Head Crater (NOAA Oahu LiDAR, EPSG:6634) was loaded as the single source. The viewer log:

```
window: 115×105 at 31.8m/px, elev -9999–240m
```

The base tier picked the **deepest overview** (115×105 at 31.8 m/px) of a dataset whose native resolution is 1 m/px. The user gave us a 12 megapixel tile and we threw 99.7 % of it on the floor for the base tier — even though the *entire* dataset is smaller than the base tier's radius.

## Why this happens

`select_ifd` (`src/viewer/tiers.rs:215`) walks IFDs from finest to coarsest and returns the first level where:

```rust
scale >= min_scale_m && window_px <= max_px
```

For the base tier the caller passes the configured `min_scale_m` from the active `VramClass` (Mid preset: ~32 m for base), and `max_px = GPU_SAFE_PX = 8192`. The function happily picks the first IFD whose pixel scale is ≥ 32 m — i.e. the 31.8 m/px overview, regardless of whether the radius window even covers more terrain than the source contains.

That logic is *correct* for the original assumption — a large source where the base tier should not waste VRAM on a finer overview than its radius needs. It breaks the moment the source is smaller than the radius.

## Why it isn't visually obvious

The close and fine tiers still pick their finer IFDs (the streamer asks for ~5 m and ~1 m min-scale, and IFD 0 satisfies both), so within their radii the user sees full-resolution data. The base tier only matters where the close/fine tiers don't cover — i.e. outside ~14 km. For a 3.6 km dataset, *the base tier is the only thing visible at the edges of the loaded extent*, and it's running at 32 m/px when 1 m was available. The visible artefact is a soft, low-detail surround that flips to crisp detail as the camera enters the close window.

## The design question

What should the base tier do when the entire source dataset is smaller than its target radius?

There are three reasonable answers, and they aren't mutually exclusive.

### Option A — Skip overviews when the whole dataset fits

If `source_extent_m < 2 * base_radius_m` (i.e. the whole source already fits in the base window), `select_ifd` should pick the *finest* IFD that still fits in `GPU_SAFE_PX`, instead of the coarsest IFD that satisfies `min_scale_m`.

For Diamond Head: full source is 3657×3348 ≤ 8192 → load IFD 0 at 1 m/px directly. The base texture becomes 3657×3348 R16Float = ~23 MB — cheap, and the close/fine tiers become **redundant** rather than additive.

This is the natural fix and matches user intent ("I gave you a 1 m source, render it at 1 m").

### Option B — Detect single-tile, small-source mode and disable the multi-tier streamer entirely

The 3-tier pipeline exists to make 100 km radii tractable. For a < 10 km dataset, the right code path is "load everything once into the base tier and never reload." This avoids spinning up the close and fine workers that will only ever serve their tiny radius and then idle.

Concretely: detect `source_extent_m < 2 * close_radius_m` at load time. If true, single-tier mode — load the whole source into the base tier at the finest IFD that fits in `GPU_SAFE_PX`, no close/fine workers.

This is cleaner than A but is a larger code change (it touches `scene_init` and `BevBaseState::new`).

### Option C — Leave it as is, but document a "single small tile" workflow

The user can already override per-tier paths via `config.toml::demo_view`. If we treat the 1 m source as both `base_tile_paths` and `fine_tile_paths`, the close/fine tiers would load it at 1 m, and the base picks the same source's coarse overview — which is *exactly the current behaviour*, just with all three tiers pointed at one file.

This is the no-code-change option. It's also unsatisfying because it requires the user to know about the multi-tier model and hand-edit TOML to work around a behaviour they didn't ask for.

## Recommended path

Option A is the smallest change that fixes the user-visible problem. It is a one-line modification to `select_ifd` (or a wrapper call site) — preferred for now.

Option B is the right long-term answer because it also stops the OS from paying for idle workers and removes the close/fine reload churn on data that doesn't need it. Worth doing when we next touch `BevBaseState::new`.

## Pre-requisite for Option B: `extract_window` honest behaviour

If the base tier holds the entire source, the close/fine tier disable path needs to handle the "no close window" case without triggering the OOM-degradation banner or the missing-tier warning in the HUD. The current `bev_base.fine = None` path is already a clean precedent — extend it to `bev_base.close = None` for the small-source case.

## Out of scope

This document is not about the **mip-count crash** the Oahu tile also exposes (`InvalidMipLevelCount { requested: 8, maximum: 7 }`) — that's a separate, mechanical bug fixed independently. The mip-count fix unblocks render; this document is about whether the render that comes back is the one the user actually wanted.
