# Single-Extract Plan: DGM_R5.tif as Primary Heightmap

## The Core Problem

Mixing two data sources (SRTM 30m + BEV 5m) at close range causes visible seam artifacts:
same mountain appears twice, perceived southward shift. This is inherent to different surveys
having different peak positions — not a code bug.

The seam is only visible when the switchover between tiers is inside the fog-free range (< 60km).
Push it past 60km and it is invisible regardless of disagreement between sources.

## Available Data

`DGM_R5.tif` — BEV DGM, whole Austria, 5m native resolution, 9 overview levels:

| IFD | Dimensions    | Resolution | 6000-px window covers |
|-----|---------------|------------|----------------------|
| 0   | 120001×70001  | 5 m        | 30 km × 30 km        |
| 1   | 60001×35001   | 10 m       | 60 km × 60 km        |
| 2   | 30001×17501   | 20 m       | 120 km × 120 km      |
| 3   | 15001×8751    | 40 m       | 240 km × 240 km      |

Fog cuts off at 60 km. IFD 1 (10m) at 6000×6000 px covers the entire fog-visible range.
IFD 2 (20m) at 3000×3000 px also covers it with room to spare.

## Clean Alternative: One Extract, One Texture

Drop the entire `hm5m_*` GPU/shader/thread infrastructure. Replace with a single
`extract_window(DGM_R5.tif, centre, 60_000m, ifd_level=1, EPSG:31287)`:

- 6000×6000 px at 10m/px — fits within the 8192 wgpu texture limit
- Covers the full fog-visible range from a single source
- No seam artifacts, no source disagreement
- Reload on drift (background thread, same pattern as the AO recompute)
- SRTM 3×3 stays as the far fallback but is entirely inside the fog band — seam invisible

Gains: no seam artifacts, no multi-bind-group complexity, no second background thread,
no extra uniform fields, no branching in the shader.

Loses: 5m close-range detail (10m is still 3× better than SRTM 30m). The 5m near-zone
can be added back later as a clean second tier once the single-source base works correctly.

## Implementation

1. Revert all `hm5m_*` additions: GPU resources, bind group entries, `CameraUniforms` fields,
   shader bindings and priority chain, viewer thread and drift check.

2. Replace the SRTM 3×3 assembled grid with a single `extract_window` from `DGM_R5.tif`:
   - `extract_window(path, centre_crs, 60_000.0, 1, 31287)` → ~6000×6000 at 10m/px
   - Upload as the primary `hm_texture` (binding 0)
   - Normals + shadow computed once; reload on drift > ~10 km

3. Keep SRTM 3×3 loading code but only activate it when outside BEV coverage
   (or drop it entirely for now — fog hides it).

4. The `extract_window` IFD fix (read geo-tags from IFD 0, scale by dimension ratio)
   is still needed and stays.

## Future: Adding 5m Near-Zone Back

Once the single-source base is stable, the 5m near tier can be added cleanly:
- `extract_window(DGM_R5.tif, centre, 5_000m, ifd_level=0, 31287)` — same file
- No source disagreement between the two tiers
- Seam at 5km is a resolution increase only (10m → 5m from same pixels)
- The `hm5m_*` infrastructure can be re-added at that point
