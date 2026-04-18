# Viewer Improvements Plan

Incremental improvements to the interactive viewer (`--view`). Each item is self-contained
and can be tackled independently. Items are ordered roughly by complexity, not priority.

---

## 1 — Ambient Occlusion (AO)

**What:** darken terrain that receives less ambient (scattered sky) light due to surrounding
geometry blocking the upper hemisphere — valleys and crevices darker, ridges brighter.
Implemented as six switchable modes cycled with the `/` key.

**HUD display:** a label in the settings HUD showing the current mode, e.g.:
```
AO: Off           (Press / to change)
AO: SSAO ×8      (Press / to change)
AO: SSAO ×16     (Press / to change)
AO: HBAO ×4      (Press / to change)
AO: HBAO ×8      (Press / to change)
AO: True Hemi    (Press / to change)
```
Six states cycle in order: Off → SSAO×8 → SSAO×16 → HBAO×4 → HBAO×8 → True Hemisphere → Off → ...
`ao_mode: u32` field on `Viewer` (0=Off, 1=SSAO×8, 2=SSAO×16, 3=HBAO×4, 4=HBAO×8, 5=TrueHemi).
`/` key (`KeyCode::Slash`) increments and wraps via `rem_euclid(6)`.
`ao_mode` is uploaded to the scene uniform (same uniform that carries camera/render parameters) and
read in the fragment shader via a uniform branch — zero divergence cost since all threads in a wave
share the same mode. The settings HUD label is updated each frame from the stored value.

**Settings HUD:** a new HUD panel (separate from the sun/clock HUD) that will accumulate render
settings over time. For now it shows only the AO label; future items (LOD mode, shadow quality,
etc.) will be added here.

---

### Mode A — Off
No AO. Baseline for comparison and fps measurement. AO factor = 1.0 everywhere.

---

### Mode B — SSAO ×8 and ×16 (Screen-Space Ambient Occlusion)

**What:** for each fragment, sample N random points in the hemisphere oriented along the
surface normal. For each sample, check if it is occluded by nearby geometry using the
depth buffer (or in this case, by comparing the sample's height against the heightmap).
Count occluded samples → fraction darkens the pixel.

**Computed:** in the raymarching fragment shader, at the hit point, each frame (real-time).
Sample count N is read from the scene uniform (`ao_mode == 1` → N=8, `ao_mode == 2` → N=16).
One shader, uniform branch — no pipeline swap, no wave divergence.

**Key parameters:**
- `N` = 8 (mode 1) or 16 (mode 2) — the split modes make the scaling curve directly measurable
- `radius` = hemisphere sample radius in world units (~50–200m for terrain scale)
- Noise/rotation kernel to avoid banding — rotate sample pattern per pixel using
  `fract(sin(dot(uv, vec2(12.9898, 78.233))) * 43758.5453)`

**Hardware angle:**
- N texture fetches per fragment per frame — measurable fps cost vs Off baseline
- Random access pattern → low texture cache hit rate; does fps drop scale linearly with N,
  or does doubling N more-than-double the miss rate? (L2 pressure compounds)
- The interesting cross-system question: M4 has a large unified L2 — does it absorb the random
  fetches better than GTX1650's smaller GDDR6 cache?
- Screen-space limitation: geometry outside the current view is invisible to SSAO;
  flying near a cliff face that's off-screen will show incorrect (too-bright) AO

---

### Mode C — HBAO ×4 and ×8 (Horizon-Based Ambient Occlusion)

**What:** instead of random hemisphere samples, march outward in D directions from the hit
point and find the maximum elevation angle (horizon angle) in each direction.
AO factor = how much those horizons block the upper hemisphere.

**Computed:** in the fragment shader, real-time.
Direction count D is read from the scene uniform (`ao_mode == 3` → D=4, `ao_mode == 4` → D=8).
One shader, uniform branch — same approach as SSAO.

**Key parameters:**
- `D` = 4 (mode 3, cardinal + diagonal ×4) or 8 (mode 4, finer angular spacing)
- `steps_per_direction` = march step count per direction (~8–16)
- `max_radius` = maximum march distance in world units

**How it differs from SSAO:**
- SSAO samples the hemisphere volumetrically (random 3D points)
- HBAO samples the horizon profile directionally (elevation angles)
- HBAO is more physically accurate and has fewer "floating shadow" artifacts
- HBAO is also more expensive: D directional marches vs N point samples

**Hardware angle:**
- Directional march is a structured (not random) access pattern → better cache behaviour
  than SSAO for the same sample count
- The non-cardinal march directions hit the heightmap at diagonal strides — the same
  cache-unfriendly pattern measured in Phase 3 (diagonal shadow 2.4× slower than cardinal)
- Measurable: compare HBAO ×4 vs ×8 fps to isolate the diagonal direction penalty
- Also compare HBAO ×4 vs SSAO ×8 — same total fetch count, different access pattern

---

### Mode D — True Hemisphere Occlusion (Precomputed)

**What:** for each terrain point, sample the full upper hemisphere by running the DDA
horizon sweep in N directions (e.g. 16). Average the unoccluded sky fraction → single
float per terrain cell, baked into a texture at startup.

**Computed:** once on the CPU at load time, stored as a `R8Unorm` texture uploaded to GPU.
At render time: one texture fetch per fragment, multiplied into the shading result.

**Relationship to existing shadow code:**
The sun shadow sweep is the single-direction specialisation of this. Full AO = run
`compute_shadow_neon_parallel_with_azimuth` in 16 evenly-spaced azimuth directions,
accumulate `(1 - shadow_factor)` for each direction, average.

**Key parameters:**
- `N` = number of directions (8 minimum, 16 good, 32 high quality)
- `penumbra_meters` — same soft-shadow parameter already in the shadow code

**Hardware angle:**
- N directional sweeps × full terrain = N × ~1.5ms on M4 NEON → ~24ms for N=16 at startup
  (acceptable one-time cost; stored in a 3601×3601 R8Unorm texture = 13 MB)
- Access pattern is a mix of cardinal (fast) and diagonal (2.4× slower) sweeps —
  total cost ≈ N_cardinal × 1.5ms + N_diagonal × 3.6ms; measurable per direction count
- At render time: one `textureSample` call → effectively free (absorbed into existing fetch cost)
- Parallelism: each output row is independent → same `rayon` structure as existing shadow

**Render-time cost:** ~0 (one texture lookup). Startup cost: O(N × terrain_size).

---

### Comparison experiment

Once all six modes work, measure fps at a fixed view. Plot quality vs cost tradeoff.
Expected result:

| Mode         | Expected fps     | Expected quality | Notes |
|---|---|---|---|
| Off          | baseline         | none             | Reference |
| SSAO ×8      | small drop       | banding visible  | Random fetch, low cache hit |
| SSAO ×16     | larger drop      | less banding     | Does cost scale 2× or more? |
| HBAO ×4      | ~SSAO ×8 cost    | better quality   | Structured fetch, same count |
| HBAO ×8      | measurably slower| best real-time   | Diagonal stride penalty |
| True Hemi    | same as Off      | best overall     | One texture fetch at render time |

Key questions the experiment answers:
- Does SSAO ×16 cost exactly 2× SSAO ×8, or does cache pressure compound?
- Is HBAO ×4 faster or slower than SSAO ×8 for the same fetch count? (access pattern effect)
- Does the HBAO ×4→×8 gap match the 2.4× diagonal penalty measured in Phase 3?
- On GTX1650 (smaller GPU cache) is the SSAO random-access penalty larger than on M4?

---

## ~~2 — Out-of-core tile streaming~~

> **Moved to `docs/planning/viewer-phase-8.md` (item 5).**

~~**What:** support terrain datasets larger than GPU VRAM by keeping only the tiles
visible from the current camera position resident on the GPU.~~

~~**Why it's interesting at the hardware level:**~~
~~- Exposes the CPU→GPU upload bandwidth as a distinct bottleneck (separate from shader compute)~~
~~- On M4 unified memory, "upload" is a pointer hand-off — near zero cost~~
~~- On discrete GPU (PCIe), a 26 MB tile upload ≈ 1.7ms; 4 new tiles/frame = 6.8ms stall~~
~~- The measurable question: at what camera speed does streaming become the bottleneck?~~

~~**Key concepts to build:**~~
~~1. **Spatial tiling on disk** — world divided into fixed-size chunks (e.g. 256×256 height
   samples), each a seekable region. Camera position maps to a set of visible chunk coords.~~
~~2. **GPU tile cache** — fixed-size 2D texture array on the GPU (e.g. 8×8 = 64 slots).
   LRU eviction when a new tile is needed and all slots are full.~~
~~3. **Indirection table (page table)** — small texture the shader consults:
   `tile_id → slot_index`. If tile not resident, render a lower-res fallback or flag the pixel.~~

~~**This is what GPU hardware calls:** sparse/virtual textures ("tiled resources" in DirectX,
"sparse binding" in Vulkan). Building it manually makes the abstraction concrete.~~

~~**Prerequisite:** multi-tile SRTM data (currently only one tile loaded).~~

---

## 3 — Level of Detail (LOD)

**What:** render distant terrain with less precision than close terrain, saving GPU work
proportional to how little detail the eye can resolve at that distance.

**The perceptual argument:** at 500m a DEM cell (~20m) = ~40 screen pixels — every crease
visible. At 50km a cell = 0.008 pixels — 125 DEM cells per pixel, 124 thrown away. Full-
precision raymarching for information that contributes nothing visible.

**Why it's interesting at the hardware level:**
- Distant rays iterate many steps through low-information terrain; each step is a texture fetch
- Many fetches hit the same few DEM cells — wasted texture cache bandwidth on redundant samples
- Smaller mip level fits better in GPU L2 cache → fetch count drops, hit rate rises
- On M4 (large unified L2) the gain may be modest; on GTX1650 (GDDR6, smaller cache) expect a clearer win
- The measurable experiment: fix a long view distance, compare fps with forced mip 0 vs computed mip level

**Three approaches in order of complexity:**

1. **Step-size LOD** *(easiest, 2-line shader change)*
   Make `min_step` grow with horizontal distance from camera:
   `min_step = base_step * (1 + t / 5000.0)`. Far rays take fewer, coarser steps.
   Sphere tracing already does this implicitly for vertical distance — extend it horizontally.

2. **Mipmap resolution LOD** *(natural next step)*
   Keep multiple downsampled heightmap versions on the GPU (classic mipmaps).
   Close rays → full-res 3601×3601; beyond 10km → 1800×1800; beyond 30km → 900×900.
   `textureSampleLevel(hm, sampler, uv, lod)` already supports this — lod is the third arg.
   Cost: 33% more GPU memory (geometric series 1 + 1/4 + 1/16 + ... ≈ 1.33×).

3. **Geometry LOD** *(not applicable here)*
   Reduce triangle count at distance. Not applicable to heightmap raymarching — no triangle mesh.
   This is what Unreal Nanite / id megatexture do; requires a fundamentally different architecture.

**Recommended order:** step-size LOD first (builds directly on existing sphere tracing),
then mipmap LOD (teaches GPU mip system concretely and is measurable on all 4 test machines).

---

