# Performance improvements — opportunities

Audit of the GPU render path (`crates/render_gpu/src/shader_texture.wgsl` +
`crates/render_gpu/src/scene/mod.rs`). Ranked by expected impact.

## 1. Hierarchical (Hi-Z) sphere tracing — TRIED, STRUCTURALLY LIMITED

**Status (updated 2026-05-02):** implemented end-to-end on Intel MacBook (4K
Hintertux scene). Did not deliver the predicted 3–5× win. **13 fps → 12 fps**
(slight regression). Per-pixel debug visualisation revealed why.

The infrastructure (`write_hm_mips` at scene/mod.rs:189, 7 max-filter mip
levels, R16Float, `MipmapFilterMode::Linear` sampler) is already in place. It
is used in the shader as an LOD bias for anti-aliasing the height sample:

```wgsl
let mip_lod = log2(1.0 + t / lod_mip_div);
var h: f32 = textureSampleLevel(hm_tex, hm_sampler, uv, mip_lod).r;
```

But the pyramid is **not** used to skip empty space — that was the proposed
optimisation.

### What hierarchical traversal does (the theory)

At mip level L, one texel covers `2^L × dx` metres. A max-filtered mip stores
the maximum height in that footprint.

- If `pos.z > h_max(L) + safety`: ray cannot intersect terrain anywhere in the
  current footprint — step by the full footprint size (potentially km) without
  missing a hit.
- If close enough that the ray *might* hit: drop to mip L-1 and re-test.
- At mip 0 with a near-surface ray: do the existing linear march + binary refine.

The original prediction was: ~500 steps × 20 m → 30–80 steps with Hi-Z, giving
**5–10× on distant rays, 3–5× overall**. Phase 7's 21 fps at 8000×2667 was
loop-dominated, and Hi-Z attacks the dominant term.

### What actually happens — the multi-tier blocker

The renderer has three height tiers: base (Copernicus 30 m, 10800×10800
assembled grid with a max-mip pyramid) plus two close-tier overlays — `hm5m`
(BEV DGM 5 m, ~18 km × 18 km centred on camera) and `hm1m` (BEV DGM 1 m, 7 km × 7
km centred on camera). The close tiers have **no mip pyramids — only mip 0**.

Where a close tier is active, the base-tier max-mip's `h_max` is **not a safe
upper bound on terrain height**: the 5 m / 1 m overlay can carry sharper peaks
not represented in the 30 m base mip. So Hi-Z cannot trust the base
max-pyramid in any region overlapped by an active close tier — including a
margin equal to the current mip-cell footprint (otherwise a coarse cell that
straddles the AABB edge could overshoot a peak the close tier holds).

The implemented gate clamps the working mip per-iteration:

```wgsl
let safe_mip = i32(clamp(
    floor(log2(max(2.0 * d_tier / cam.dx_meters, 1.0))),
    0.0, 7.0));
mip = min(mip, safe_mip);
```

…where `d_tier` is the distance from `pos.xy` to the nearest active close-tier
AABB (minus a 50 m safety margin). Inside the AABB or its margin,
`safe_mip = 0` and the iteration drops to the existing linear/sphere-traced
branch.

### What the visualisation showed

A debug pass colour-codes each pixel by per-iteration counters:

- **R** = Hi-Z skips (footprint jumps over empty space)
- **G** = Hi-Z descents that couldn't skip + linear iterations *outside* the
  tier (post-descent fallback) — i.e. ray is below local peaks
- **B** = linear iterations *inside* the tier (`safe_mip = 0`, geometrically
  forced)

Result on the user's view (`docs/planning/tmp/hz.png` reference snapshot):

- **Sky = pure red.** Hi-Z is fully effective on upward / grazing rays — but
  these were already cheap (sky early-exit at 10000 m).
- **Yellow horizon band** (R + G). Terrain at >9 km from camera hits *outside*
  the 5 m tile; Hi-Z engages but mixes skips with descents. Some win, some
  waste.
- **Foreground & mid-distance terrain = pure blue.** Rays terminate *inside*
  the 5 m tile and never escape its exclusion zone. Hi-Z is geometrically
  inapplicable to these pixels — and they are the frame-budget dominator.

### Why the doc's 3–5× estimate was wrong

The estimate assumed the only data structure was the base tier with its
max-mip pyramid. With centred close-tier overlays covering most of what a
user actually looks at, Hi-Z can run only in the *minority* of pixels (sky and
the thin horizon band). The dominant pixels' rays sphere-trace through the
tier zone exactly as before. Per-iteration overhead added by the Hi-Z gate
(`safe_mip` log2, `dist_outside_tier_xy` sqrt, the clamp) runs on every
blue-zone iteration without ever firing the win path → small net regression.

### Lesson

Algorithm wins are conditional on scene geometry. Hi-Z is the right
optimisation when rays have lots of headroom over a single trusted height
field. With multi-resolution overlays centred on the camera, the trusted
field is not the one Hi-Z would jump over.

### Two paths forward — pick before retrying Hi-Z

**A. Build mip pyramids on hm5m and hm1m too.** Drops the `safe_mip`
restriction entirely — Hi-Z can run at any mip including inside the tier.
Cost: ~half a day; max-mip generation already exists for the base tier
(`write_hm_mips`) and ports to the close tiers; bind-group entries for the
close-tier mips and a per-tier mip-aware sampler are needed; memory cost
~1.33× per tier (fine on 7000² and 8000² grids). This is the proper unblock —
Hi-Z's *full* claimed value depends on it.

**B. Skip Hi-Z entirely for now.** Hi-Z's current standalone value is small
(sky and horizon already cheap) and the per-iteration tax in the blue zone
is real. Items 3 (`max_terrain_h` for tighter sky exit) and 4 (binary search
32 → 10 iters) attack work that *every* ray does, including the dominant
blue-zone rays. Smaller per-item gains, but they actually land. Recommended
short-term sequencing.

### Cell-exit math (kept here for whoever reattempts after path A)

The implementation used proper cell-exit stepping (better than the doc's
original `t += pow(2, mip) * dx` simplification — that overshoots cells with
short ray crossings):

```wgsl
fn cell_exit_dt(pos: vec3<f32>, dir: vec3<f32>, mip: i32, dx_m: f32) -> f32 {
    let cell = exp2(f32(mip)) * dx_m;
    let cx = floor(pos.x / cell);
    let cy = floor(pos.y / cell);
    let bx = (cx + select(0.0, 1.0, dir.x > 0.0)) * cell;
    let by = (cy + select(0.0, 1.0, dir.y > 0.0)) * cell;
    let tx = select(1e30, (bx - pos.x) / dir.x, abs(dir.x) > 1e-6);
    let ty = select(1e30, (by - pos.y) / dir.y, abs(dir.y) > 1e-6);
    return max(min(tx, ty), 1e-2);
}
```

References for the standard algorithm (Maximum Mipmap Sphere Tracing / Hi-Z
trace):
- Tevs et al. "Maximum Mipmaps" (2008)
- Mara/McGuire "Efficient GPU Screen-Space Ray Tracing" (2014)

## 2. Direct swap-chain write (eliminate output buffer copy)

`render_frame` (scene/mod.rs:767) writes to `output_buf` (storage), then
`copy_buffer_to_buffer` to `readback_buf`, then maps for CPU read. This is the
benchmark/single-image path — readback is unavoidable there.

`dispatch_frame` (scene/mod.rs:884) is the viewer path — but it **also** writes
to `output_buf`. The viewer must then do another buffer→texture copy (or use
`output_buf` as a sampled texture, which means the swap chain has to format-
convert during compositing).

**Better:** rebind binding 3 as `texture_storage_2d<bgra8unorm, write>` pointing
at the swap chain image, use `textureStore(output, vec2<i32>(gid.xy), color)`.

### Impact

- Apple Silicon (unified memory): minor — the buffer→texture copy is fast
- Discrete GPU (GTX 1650 etc.): 2–5 ms/frame at 1600×533 saved (Phase 7 noted
  PCIe was 96% of frame time before swap-chain was added; need to verify the
  current viewer path actually skips the buffer round-trip)

### Verification first

Before changing anything, instrument the viewer path. If `dispatch_frame` is
already feeding the swap chain via a different bind group, this point is moot.
If it's still going through `output_buf` → swap-chain copy, fixing it is
straightforward.

## 3. Tighter sky early exit

```wgsl
// shader_texture.wgsl:265
if dir.z > 0.0 && pos.z > 10000.0 { break; }
```

For a camera at 2000m looking up at any non-grazing angle, the ray climbs to
10km before this triggers — hundreds of wasted steps for sky pixels.

**Fix:** pass `max_terrain_height` (computed once from the top mip level) in
the camera uniform. For Hintertux that's ~3700m, not 10000m.

```wgsl
if dir.z > 0.0 && pos.z > cam.max_terrain_h + 100.0 { break; }
```

### Impact

60–70% reduction in sky-ray step count. Typically 30–50% of pixels are sky in
a typical alpine view. Net: probably 10–20% overall fps gain.

Trivial to implement: one float in `CameraUniforms`, one `max()` reduction
over the top mip level when the heightmap is uploaded.

## 4. Reduce `binary_search_hit` iterations from 32 to ~10

```wgsl
// shader_texture.wgsl:300
pos = binary_search_hit(t_prev, t, dir, 32);
```

`binary_search_hit` does up to 32 iterations × up to 3 texture samples each
(base + hm5m + hm1m via `sample_h_exact`) = **up to 96 samples per pixel just
for hit refinement**.

Math: each iteration halves the bracket. Starting from `t_hi - t_lo ≈ step_m
≈ 20m`, after 10 iterations the bracket is `20 / 2^10 ≈ 0.02m` — well below
sub-pixel error at any realistic camera distance.

**Fix:** change `32` to `10`. Free win, no visual change.

### Impact

3× reduction in hit-refinement cost. Hit refinement is one-shot per pixel
(only on hit), not per-step, so this is small relative to Hi-Z but trivial.

## 5. Eliminate divergent inner-loop branches

```wgsl
// shader_texture.wgsl:280
let in_close_l = cam.hm5m_extent_x > 0.0 && lx_loop >= 0.0 && ...;
if in_close_l {
    // sample hm5m, smoothstep, mix
}
```

`cam.hm5m_extent_x > 0.0` is a uniform branch (free). But `in_close_l` is
**per-thread divergent** — when the camera straddles a tier boundary, half the
workgroup samples 3 textures, half samples 1, and the whole subgroup waits for
the slow path.

### Two possible fixes

**a) Per-workgroup AABB pre-test:** at the start of `main()`, compute the
`t_enter_5m` and `t_exit_5m` for the workgroup's average ray. Inside that t
range every thread does the tier sample (no divergence); outside, no thread
does. Reduces "divergent decision" to a uniform-ish range check.

**b) Force all threads through both paths:** sample hm5m unconditionally, but
mask to 0 when outside the tier. Wastes ~5% extra samples but eliminates
divergence stalls. Often the right call when divergence is high.

### Impact

Probably 10–25% on top of Hi-Z when the camera is near tier edges (the common
case for the Hintertux scenario). Less impactful when fully inside or outside
a tier.

## 6. Pack base tier normals (12 storage loads → 4)

`_nx_buf`, `_ny_buf`, `_nz_buf` (bindings 4–6) are still three `array<f32>`
storage buffers. Per-pixel hit shading does **12 unindexed buffer loads** (4
corners × 3 components):

```wgsl
// shader_texture.wgsl:341–344
let n_base = normalize(mix(
    mix(vec3<f32>(nx[i00], ny[i00], nz[i00]), vec3<f32>(nx[i10], ny[i10], nz[i10]), fx),
    mix(vec3<f32>(nx[i01], ny[i01], nz[i01]), vec3<f32>(nx[i11], ny[i11], nz[i11]), fx), fy
));
```

This couldn't move to a 2D texture (>8192px Copernicus grid), but two options:

**a) `texture_2d_array<f32>` with 4 layers** of 8192×8192 covering the
10800×10800 grid with overlap. Picks slice based on (col, row). Gets cached
sampling.

**b) Packed `u32` storage buffer:** `(nx_i16 << 16) | ny_i16` per pixel.
Reconstruct `nz = sqrt(1 - nx² - ny²)` in the shader (same trick we did for
hm5m/hm1m). 12 loads → 4 loads, 12 bytes/px → 4 bytes/px.

Memory: 1.4 GB → 466 MB on the 10800×10800 base grid (3× reduction).

### Impact

Moderate. Base tier hit shading is one-shot per ray, not per-step, so this is
small relative to Hi-Z. But the memory savings are significant if you ever
want to hold multiple base grids simultaneously (e.g., for tile-slide
overlap).

## 7. Specialization constants for runtime mode flags

The shader has runtime branches for `ao_mode`, `shadows_enabled`, `fog_enabled`,
`vat_mode`, `lod_mode`. These are uniform-but-not-compile-time, so the compiler
can't dead-code-eliminate.

The chained `select` for LOD parameters is particularly bad:

```wgsl
// shader_texture.wgsl:272
let lod_step_div = select(select(select(4000.0, 8000.0, cam.lod_mode < 3u), 20000.0, cam.lod_mode < 2u), 1000000000.0, cam.lod_mode == 0u);
```

This compiles to a chain of compares **per loop iteration** (executed 500
times per pixel). Two fixes:

**a) Quick fix:** pass `lod_step_div` and `lod_mip_div` as `f32` in the camera
uniform, computed CPU-side once per frame. Removes the chain of compares from
every loop iteration.

**b) Proper fix:** WebGPU pipeline-overrideable constants (or specialization
constants in Vulkan/Metal). Compile 4 specialised pipelines (one per LOD
mode), select on the CPU. Eliminates the branches entirely.

### Impact

(a) is a few percent and trivial. (b) is more significant on lower-end GPUs
where instruction throughput matters more, but adds pipeline-management
complexity.

## Suggested order of work

1. **Hi-Z hierarchical traversal** — biggest win, infrastructure exists. ~1 day.
2. **Tighter sky exit** — trivial, ~30 min.
3. **Reduce `binary_search_hit` to 10 iterations** — trivial, ~5 min.
4. **Verify viewer path / direct swap-chain write** — investigation first.
5. **Inner-loop divergence fix** — only after Hi-Z, since Hi-Z changes the loop structure.
6. **Pack base normals** — when you next touch that area.
7. **Specialization constants** — only after measuring whether it matters.

## What NOT to do

- Don't chase micro-optimisations (instruction count tweaks, FMA fusion) until
  the big wins are landed. Hi-Z dominates everything else by 10×.
- Don't rewrite the WGSL into HLSL/MSL hand-tuning — the wgpu/Naga pipeline
  produces reasonable code; the wins are algorithmic, not code-gen.
- Don't add more AO modes / lighting features until the perf foundation is in
  place. They'll just compound the overhead.

## Verification methodology

For each change:
1. Run `cargo run --release -- --view --1m-tiles-dir <path>` with the same
   camera position and sun angle.
2. Read FPS from the HUD.
3. Compare against baseline at three view distances:
   - close (camera 200m above terrain, looking down at 30°)
   - mid (camera 2000m, looking at horizon)
   - far (camera 5000m, looking at horizon — sky-heavy)

The "far" case is where Hi-Z and the sky-exit fix should show up most.
