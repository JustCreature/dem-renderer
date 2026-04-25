# Viewer Phase 8 Plan

Incremental viewer improvements. Each item is self-contained.

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

## 5 — Out-of-core tile streaming

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
