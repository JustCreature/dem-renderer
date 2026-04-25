# Viewer Improvements Plan

Incremental improvements to the interactive viewer (`--view`). Each item is self-contained
and can be tackled independently. Items are ordered roughly by complexity, not priority.

---

## 1 — Ambient Occlusion (AO)

**What:** darken terrain that receives less ambient (scattered sky) light due to surrounding
geometry blocking the upper hemisphere — valleys and crevices darker, ridges brighter.
Implemented as three switchable modes cycled with the `/` key.

**HUD display:** a label showing the current mode, e.g.:
```
AO: Off          (Press / to change)
AO: SSAO         (Press / to change)
AO: HBAO         (Press / to change)
AO: True Hemi    (Press / to change)
```
Four states cycle in order: Off → SSAO → HBAO → True Hemisphere → Off → ...
`ao_mode: u8` field on `Viewer` (0=Off, 1=SSAO, 2=HBAO, 3=TrueHemi).
`/` key (`KeyCode::Slash`) increments and wraps via `rem_euclid(4)`.
The current mode name is uploaded to the uniform and the HUD label updated each frame.

---

### Mode A — Off
No AO. Baseline for comparison and fps measurement. AO factor = 1.0 everywhere.

---

### Mode B — SSAO (Screen-Space Ambient Occlusion)

**What:** for each fragment, sample N random points in the hemisphere oriented along the
surface normal. For each sample, check if it is occluded by nearby geometry using the
depth buffer (or in this case, by comparing the sample's height against the heightmap).
Count occluded samples → fraction darkens the pixel.

**Computed:** in the raymarching fragment shader, at the hit point, each frame (real-time).

**Key parameters:**
- `N` = number of samples (8–16 is typical; trade quality vs cost)
- `radius` = hemisphere sample radius in world units (~50–200m for terrain scale)
- Noise/rotation kernel to avoid banding — rotate sample pattern per pixel using a small
  noise texture or `fract(sin(dot(uv, vec2(12.9898, 78.233))) * 43758.5453)`

**Hardware angle:**
- N texture fetches per fragment per frame — measurable fps cost vs Off baseline
- Random access pattern → low texture cache hit rate; the interesting question is how
  N scales fps on M4 vs GTX1650 (different L2 sizes)
- Screen-space limitation: geometry outside the current view is invisible to SSAO;
  flying near a cliff face that's off-screen will show incorrect (too-bright) AO

---

### Mode C — HBAO (Horizon-Based Ambient Occlusion)

**What:** instead of random hemisphere samples, march outward in D screen-space directions
from the hit point and find the maximum elevation angle (horizon angle) in each direction.
AO factor = how much those horizons block the upper hemisphere.

**Computed:** in the fragment shader, real-time.

**Key parameters:**
- `D` = number of directions (4–8 typical)
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
- Measurable: compare HBAO fps with D=4 cardinal-only directions vs D=8 with diagonals

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

Once all three modes work, measure fps (Off, SSAO N=8, SSAO N=16, HBAO D=4, HBAO D=8,
True Hemi) at a fixed view. Plot quality vs cost tradeoff. Expected result:
- Off: baseline fps
- SSAO N=8: small fps drop, noticeable artifact banding
- SSAO N=16: larger fps drop, less banding
- HBAO D=4: comparable cost to SSAO N=8, better quality
- HBAO D=8 with diagonals: measurably slower than HBAO D=4 cardinal-only
- True Hemi: same fps as Off (precomputed), best quality, no view-dependent artifacts

---

## 2 — Out-of-core tile streaming

**What:** support terrain datasets larger than GPU VRAM by keeping only the tiles
visible from the current camera position resident on the GPU.

**Why it's interesting at the hardware level:**
- Exposes the CPU→GPU upload bandwidth as a distinct bottleneck (separate from shader compute)
- On M4 unified memory, "upload" is a pointer hand-off — near zero cost
- On discrete GPU (PCIe), a 26 MB tile upload ≈ 1.7ms; 4 new tiles/frame = 6.8ms stall
- The measurable question: at what camera speed does streaming become the bottleneck?

**Key concepts to build:**
1. **Spatial tiling on disk** — world divided into fixed-size chunks (e.g. 256×256 height
   samples), each a seekable region. Camera position maps to a set of visible chunk coords.
2. **GPU tile cache** — fixed-size 2D texture array on the GPU (e.g. 8×8 = 64 slots).
   LRU eviction when a new tile is needed and all slots are full.
3. **Indirection table (page table)** — small texture the shader consults:
   `tile_id → slot_index`. If tile not resident, render a lower-res fallback or flag the pixel.

**This is what GPU hardware calls:** sparse/virtual textures ("tiled resources" in DirectX,
"sparse binding" in Vulkan). Building it manually makes the abstraction concrete.

**Prerequisite:** multi-tile SRTM data (currently only one tile loaded).

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

