# Far tier — coarse-resolution horizon ring

## Motivation

The 3-tier streamer (`base` ≈ 30 m / 70–90 km, `close` ≈ 5 m / 8–20 km, `fine` ≈ 1 m / 1–3.5 km) caps visibility at the base radius. Beyond that the ray exits the loaded heightmap and falls through to the sky/fog color — even though the camera (especially at altitude) can plausibly see terrain hundreds of kilometres away.

Trying to raise the base radius hits two walls:

1. **Adapter texture-dimension limit.** `GPU_SAFE_PX = 8192` is the conservative cross-hardware floor (some adapters allow 16384). At 47 °N a 90 km E-W radius already needs ~7 200 px at 30 m/px — bumping radius to 200 km would need ~16 000 px, which exceeds the safe floor.
2. **CPU work scales with area.** Normals, shadow sweep, AO and the GPU upload bytes all scale with `radius²`. Doubling the base radius quadruples the per-reload cost, and reloads already produce a perceptible hitch on integrated GPUs.

But neither cost is justified by what the user actually sees at long distance.

### The screen-pixel-vs-source-pixel math

At a 60° horizontal FOV on a 1920-wide window, a screen pixel subtends `60° / 1920 ≈ 0.031°`. The terrain distance covered by one screen pixel at distance `D` is `D × tan(0.031°) ≈ D / 1830`.

| Distance | Terrain m / screen px | Useful source resolution |
|---|---|---|
| 10 km | 5.5 m | 5 m source (close tier) is matched |
| 30 km | 16 m | 30 m source is matched |
| 50 km | 27 m | 30 m source still slightly oversampled |
| 100 km | 55 m | 60 m source visually identical to 30 m |
| 200 km | 109 m | 90 m source visually identical to 30 m |
| 300 km | 164 m | 180 m source visually identical |
| 500 km | 273 m | 270–360 m source fine |

Past ~50 km, 30 m source is wasted — you're spending VRAM and bandwidth on detail the viewer cannot resolve. A coarser tier covering the same area uses dramatically less budget: a 90 m tier covers 3× the radius of a 30 m tier in the same texel count, and a 270 m tier covers 9× the radius.

## Proposed design

Add a fourth tier — call it `far` — wrapped around the existing base tier.

### Numbers

| Parameter | Value (proposed default, High preset) |
|---|---|
| Source pixel scale | 90 m/px (3× downsample of GLO-30) |
| Radius | 300 km |
| Window size | ~6 700 × 6 700 px at 47 °N (fits 8 192) |
| Drift threshold | 100 km (1/3 of radius, matches existing tier ratio) |
| GPU formats | R16Float heightmap + 8 mips, packed-u32 normals; **no shadow buffer, no AO** |
| VRAM cost | ~190 MB (hm 90 + normals 90) — comparable to current base tier |

Low/Mid/High presets get the same scaling as existing tiers (~0.55× / 0.78× / 1.0× radius).

Optional second far stage at 270 m/px / 900 km radius if Everest-style "see the curvature" panoramas become a goal. Skip for v1.

### What the far tier omits

- **Shadows.** A 200 km shadow sweep at 90 m/px is ~6 700² × 100 ray steps = 4.5 G ops per reload. At distance, sun shadows are also dominated by atmospheric haze and aerial perspective — the contribution to perceived shading is small. Render the far tier under a simple Lambertian `dot(normal, sun_dir)` instead.
- **AO.** Same story — AO bake is `O(area × 16 azimuths × ray_length)`, and ambient occlusion at >100 km is visually negligible against fog.
- **Fine-tier-style bicubic interpolation.** Too distant to matter, and the linear filter on the R16Float texture is enough.

These omissions are the entire point of having a separate tier — the existing base tier code already has both shadow and AO branches, and we don't want to scale them up.

### Blending with the base tier

Same `BLEND_MARGIN` (500 m) feathered transition `smoothstep` that the base/close and close/fine boundaries use. Far tier samples are active when the ray is **outside** the base tier's loaded extent, fading in over a 500 m margin so the base→far transition is invisible. At the very outer edge of the far tier, fog should already be near 100% — see "Fog tuning" below.

## Architectural impact

### Bind group

Current canonical 20-entry bind group (see CLAUDE.md "Bind Group Layout"). Adding a far tier extends it by 4 entries:

| New binding | Resource |
|---|---|
| 20 / 21 | far hm `texture_2d<f32>` (R16Float, 8 mips) + filtering sampler |
| 22 | far packed normals (`storage, read` u32 array) |
| 23 | far tier rotation / origin / extent (could be packed into `CameraUniforms` instead — see Open question 1) |

Or — pack everything into `CameraUniforms` and re-use the existing base sampler / a single new texture binding. The exact layout is a micro-decision; what matters is `rebuild_bind_group` already handles structural changes and is the single canonical assembly point.

### Shader

Mirror the existing base/close/fine blend in `crates/render_gpu/src/shader_texture.wgsl` (around line 527, the `// ── height sample: base → 5m → 1m blend ──` region). New layering, **from coarsest to finest**:

```
h = far_sample(uv_far)                              // always available, biggest extent
h = mix(h, base_sample(uv_base), base_in_mask)
h = mix(h, close_sample(uv_close), close_blend_mask)
h = mix(h, fine_sample(uv_fine), fine_blend_mask)
```

Same for normals. Shadow and AO branches stay on the existing base/close/fine tiers only — the far tier contributes only height + normals + a simple sun-dot term.

### Tile sources

90 m/px source data is straightforward — `ensure_overview_cache` (`crates/dem_io/src/overview.rs`) already builds box-averaged pyramids. Need to extend it to also produce a ~90 m level next to existing ~8 m / ~32 m levels (or add a separate `ensure_far_overview_cache`).

For the Copernicus 3×3 / 5×5 / etc. demo path, the assembled grid is currently dropped at 30 m. We'd either:

- **Option F1 (assembly-time downsample).** After `load_grid_from_paths` assembles the N×M grid, box-average to 90 m before handing to the far worker. The far tier loads from an in-memory copy, never re-reads from disk.
- **Option F2 (overview cache for Copernicus tiles too).** `ensure_overview_cache` was designed for large single-IFD tiles; extending it to mosaicked Copernicus needs care because the assembled grid isn't a file. Effectively the same work as F1, just persisted.

F1 is cheaper for a first cut.

### Worker thread

A new `FarTier` worker mirroring `BevBaseState::base` in `src/viewer/tiers.rs`:

- Holds WGS84 `(lat, lon)`, drift threshold 100 km.
- On drift, re-extracts a `2 × far_radius_m` window from the source set (or downsampled cache).
- Computes normals (skip shadow/AO).
- Pre-packs bytes (u32 normals, f16 hm + mips) on the worker.
- Sends a `FarTierData` bundle for `write_texture` / `write_buffer` on the main thread.

OOM degradation path (`render_gpu::context::on_uncaptured_error`): if VRAM runs out, **disable the far tier first** (it's the least-visually-important), then the fine, then the close. Update `Viewer::poll_oom_status` accordingly.

### TierRadii

`TierRadii` in `src/viewer/tiers.rs` gains a `far_radius_m` + `far_drift_threshold_m` field per `VramClass`. Demo TOML override section follows the existing pattern. Suggested defaults:

| VramClass | far_radius_m | far_drift |
|---|---|---|
| Low | 150 km | 50 km |
| Mid | 220 km | 75 km |
| High | 300 km | 100 km |

## Fog tuning

The current `sky_color`-driven fog (`shader_texture.wgsl`, ~line 812) already scales fog distance with altitude (`exp(alt/8000)`, capped at 6× ≈ 360 km `fog_far`). With a 300 km far tier:

- At sea level, fog_far = 60 km. Far tier covers 300 km of terrain that's entirely behind 100% fog. **Wasted load.**
- At 3 km, fog_far = 87 km. Still mostly fogged.
- At 8 km (Everest), fog_far = 163 km. Outer half of far tier is fogged.
- At ≥14 km, fog_far = 360 km. Far tier is fully visible.

So either:
- Gate the far tier load on `cam.origin.z > 2_000.0` (or similar) — don't pay the load cost when you can't see it.
- Or accept the over-load at low altitude as the cost of a uniform tier model and let the OOM degradation path drop it when memory pressure rises.

The former is cleaner.

## VRAM accounting

Per the existing `[vram] alloc tex …` instrumentation, expected adds for High preset:

```
far_hm_tex        ~92 MB  (8192² × 2 bytes × 1.33 mip overhead)
far_normals_buf   ~90 MB  (8192² × 4 bytes / 3 for cropped 6700²)
                  ─────
                   ~180 MB  on top of current ~896 MB Mid budget
```

Comfortably fits on 4 GB+ discrete; will trip OOM degradation on 2 GB Intel Iris-class integrated. That's expected and handled.

## Open questions

1. **Bind group expansion vs. uniform packing.** Cleanest in shader is a 4th texture binding pair; cleanest in CPU code might be to push the far tier's origin/extent/rotation into `CameraUniforms` and keep the bind group at 24 entries instead of also bumping the binding-group entry count. Decide when implementing.
2. **Do we need a far normal map at all?** A 90 m surface is flat-looking enough that a single per-tier "flat" normal might be acceptable. Saves the 90 MB normal buffer. Worth A/B-ing — would the user notice the loss of shading detail past 100 km?
3. **Sun shadow contribution at distance.** Once the far tier exists, we should probably blend the existing base-tier shadow buffer outward by feathering its value to "no shadow" (1.0) at the base/far boundary, not abruptly cutting off. Otherwise there's a visible shadow termination line at 90 km on a clear day.
4. **Single-tier collapse.** When the camera is low and fog hides everything past 80 km, should the far tier worker actually run? Combine with Question 4 from the open items in CLAUDE.md re: low-altitude visibility gating.

## Prerequisites and ordering

This builds on top of:

1. ✅ N×M `assemble_grid` (just landed in `crates/dem_io/src/grid.rs`) — needed so the demo path can supply enough tiles to actually fill a 300 km far ring.
2. ✅ Camera-centered crop in `prepare_demo_scene_with_ctx` (just landed in `src/viewer/scene_init.rs`) — needed so the base tier doesn't try to upload an 18000² texture from the same source pool.
3. Generic overview-cache support for assembled (non-file) grids if going with Option F2 (otherwise F1 sidesteps this).

The far tier itself is the largest single change in this list. The two prereqs were needed regardless (the user's 5×5 fog test exposed both).

## Out of scope

- Curvature-of-Earth ray bending for true 500 km+ horizon math. Becomes relevant past ~200 km. Worth its own doc.
- LOD-aware ray-step adjustment (longer steps when sampling the far tier). Probably necessary for shader perf; covered by existing `lod_step_div` switch but tuning is part of far-tier integration.
- Sky-ground horizon line (haze gradient on the sky color where it meets the terrain at the horizon). Independent shader work.
