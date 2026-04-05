# Phase 4 Session Log
**Date**: 2026-04-05
**Hardware**: Apple M4 Max
**Branch**: phase_4

---

## Session Goals

Implement a CPU raymarcher for the heightmap terrain:
- Pinhole camera model with orthonormal basis
- Heightmap raymarching with binary search refinement
- Lambertian shading + shadow mask integration + elevation color bands
- NEON SIMD packet raytracing (4-lane RayPacket, SoA)
- Rayon parallelism (scalar and NEON)
- Position camera to match a Google Earth view of the Austrian Alps
- Benchmark all 4 variants, understand bottlenecks

---

## Step 1 — Pinhole Camera Model

Built `Camera` and `Ray` structs in `crates/render_cpu/src/camera.rs`.

**Orthonormal basis**:
```
forward = normalize(look_at - origin)
right   = normalize(cross(forward, world_up))   // world_up = [0,0,1]
up      = cross(right, forward)                 // no normalization needed — already unit
```

**Ray generation** for pixel (px, py) in (W, H):
```
u     = (px + 0.5) / W
v     = (py + 0.5) / H
ndc_x = -(2u - 1)    // negated to flip horizontal (geographic convention: X east)
ndc_y =  (1 - 2v)    // Y-flip: screen Y down, camera Y up
dir   = normalize(forward + ndc_x * half_w * right + ndc_y * half_h * up)
```

**World coordinate system**: X = meters east, Y = meters south, Z = elevation meters.
Camera position: `[col * dx_m, row * dy_m, elevation]`.

**Bug caught**: initial `add()` implementation used `b[0]` for all three components (`[a[0]+b[0], a[1]+b[0], a[2]+b[0]]`). Fixed to `b[1]`, `b[2]` — caught by verifying top-left vs center ray directions manually.

---

## Step 2 — Scalar Raymarcher

Implemented `raymarch()` and `binary_search_hit()` in `crates/render_cpu/src/raymarch.rs`.

**Core loop**: step `t` from 0 to `t_max` in increments of `step_m = dx_meters ≈ 20.7m`. At each step, compute world position, convert to heightmap col/row, bounds-check, sample terrain height, compare `p.z < terrain_z`.

**Binary search refinement**: 8 bisections on interval `[t - step_m, t]`. Accuracy: `20m / 2^8 ≈ 0.08m`. Returns last confirmed above-ground position (`t_lo`).

**Coordinate bug**: initial camera setup hardcoded `21.06 m/pixel` while `hm.dx_meters = 20.691`. 1.8% error × 2388 columns = 41-column drift in world position. Fix: always use `hm.dx_meters as f32` — never hardcode cell sizes.

---

## Step 3 — Camera Placement to Match Google Earth

Target: Hintertux / Tuxer Alps area (tile n47_e011). Located Olperer peak (3449m) at row=3409, col=2370 by searching the heightmap directly.

**Discrete sampling problem discovered**: a single-pixel-wide peak can be missed when the ray threads through lower-elevation neighbor pixels. The ray's fractional XY position may never land on the peak pixel. Diagnosed by computing `t_olperer` analytically and finding the ray was 2.1m below the geographic peak but the heightmap stored 1731m at the estimated col (wrong coords). Root cause: 41-column drift from hardcoded cell sizes.

**Google Earth → world coords conversion**:
```
col = (lon - 11.0) * 3600
row = (48.0 - lat) * 3600
tilt_below = (90 - T) * π/180
look_at_x = cam_x + sin(H°) * cos(tilt_below) * dist
look_at_y = cam_y - cos(H°) * cos(tilt_below) * dist
look_at_z = cam_z - sin(tilt_below) * dist
```

**Horizontal mirror bug**: discovered image was left-right flipped vs Google Earth. Root cause: camera convention. Fix: negate `ndc_x` in `ray_for_pixel` (`-(2u - 1)`) — surgical horizontal flip only. Reversing the cross product order was tried and caused both-axis flip (image upside down) — reverted.

Final camera: 47°04'31.90"N 11°40'56.64"E, altitude 3341m, heading 85°, tilt 80°.

---

## Step 4 — Shading

Added `shade()` as `pub(crate)` in `crates/render_cpu/src/render.rs`, shared between scalar and NEON render paths.

**Elevation color bands**:
- < 1800m → `[120, 160, 80]` green
- < 2000m → `[160, 175, 130]` transitional
- < 2600m → `[200, 200, 195]` grey rock
- ≥ 2600m → `[240, 245, 250]` glacier white

**Lambertian + shadow**:
```
diffuse    = max(0, dot(normal, sun_dir))
brightness = ambient + (1 - ambient) * diffuse * shadow
color      = base_color * brightness
```

**Shadow softness**: ambient = 0.25 prevents fully black shadows (shadows are partially lit by sky). Pure `ambient = 0.0` gave unnaturally black shadows.

**Sun alignment**: `sun_dir: [f32; 3]` is the single source of truth. Both azimuth and elevation for the shadow mask are derived from it:
```rust
let azimuth   = sun_dir[0].atan2(-sun_dir[1]);
let elevation = sun_dir[2].atan2(hypot(sun_dir[0], sun_dir[1]));
```

**Shading vs shadowing distinction**:
- Shading: slope faces away from sun → dark (local, from normal map)
- Shadowing: another mountain blocks sun → dark (global, from DDA sweep)
- Stacked: `brightness = ambient + (1-ambient) * diffuse * shadow`

---

## Step 5 — NEON Packet Raytracing

Implemented `RayPacket`, `raymarch_neon()`, `binary_search_hit_neon()` in `crates/render_cpu/src/raymarch_neon.rs`.

**RayPacket SoA layout**: 6 `float32x4_t` fields (origin_x/y/z, dir_x/y/z). SoA allows `vmulq_f32(dir_x, t_vec)` to advance all 4 X components in one instruction.

**Active mask**: `uint32x4_t`, initialized `vdupq_n_u32(u32::MAX)`. Lanes deactivated via `vbicq_u32(is_active, new_hits)` (= `a & ~b`).

**Bounds check in SIMD**: `vcltq_f32` / `vcgeq_f32` return `uint32x4_t` directly on AArch64 — no reinterpret cast needed. OOB mask OR'd together and used to deactivate lanes via `vbicq_u32`.

**Gather**: no NEON gather instruction — 4 scalar loads + `vld1q_f32` pack. OOB lanes clamped before indexing: `c.clamp(0, cols-1)`.

**NEON binary search**: per-lane `t_lo`/`t_hi` as `float32x4_t`. `vbslq_f32(mask, a, b)` updates bounds per lane independently. Guarded with `if vmaxvq_u32(new_hits) != 0` — avoids 8 wasted gathers per step on the ~505 non-hit steps.

**Bugs encountered**:
- `vdupq_n_f32(0xFFFFFFFF)` — wrong type for active mask; fixed to `vdupq_n_u32(u32::MAX)`
- `vaddq_f32` instead of `vmulq_f32` for `p = origin + t * dir` — caught by all-blue output
- `vgetq_lane_f32` with variable index — NEON lane index must be compile-time const; unrolled 4 explicit calls with `:<0>`, `:<1>`, etc.
- Binary search called unconditionally — wrapped in `vmaxvq_u32(new_hits) != 0` guard

---

## Step 6 — Rayon Parallelism

Added `render_par()` and `render_neon_par()`.

**Pattern**: `par_chunks_mut((width * 3) as usize)` divides framebuffer into one row per chunk. `enumerate()` gives `(py, row_buf)`. Each thread writes to its own row — no false sharing (rows are `width × 3 ≈ 6000` bytes >> 64-byte cache line).

**Bug**: inside the parallel closure, `framebuffer[idx]` was used instead of `row_buf[idx]`. Fixed — the closure captures the row slice, not the full framebuffer.

**`par_chunks_mut` vs `par_bridge`**: `par_chunks_mut` is natively parallel for slices with no overhead. `par_bridge` converts a sequential iterator to parallel — slightly more overhead; unnecessary for slices.

---

## Benchmark Results

Hardware: Apple M4 Max, 10 cores, 2000×900 image, step_m = dx_meters ≈ 20.7m

| Version | Time | vs scalar |
|---|---|---|
| Scalar single-thread | 0.80s | 1× |
| NEON single-thread | 0.80s | 1× |
| Scalar parallel (10 cores) | 0.08s | 10× |
| NEON parallel (10 cores) | 0.08s | 10× |

Average steps per ray: **506** (≈10.5 km travel at step_m=20.7m)

**Why NEON ≈ scalar (single thread)**:
- Workload is memory-bound with sequential access — prefetcher trains on the stride
- Compiler auto-vectorizes scalar `raymarch` in release mode
- Manual NEON adds: 6 `vld1q_f32` packs in `RayPacket::new`, 8 lane extracts per step, broadcast `t` each step — these costs cancel the SIMD arithmetic benefit
- Gather (4 scalar loads per step) is the true bottleneck; tiling or supersampled ray would be needed to address it

**Why parallel wins**: embarrassingly parallel, no shared mutable state, independent rows. 10× speedup from 10 cores = near-ideal scaling.

---

## Considered but Not Implemented

**Screen-space tiling**: batching screen pixels into 2D tiles for better terrain spatial locality. Not beneficial here — horizontal 1×4 NEON packets already give optimal cache-line reuse (adjacent pixels land in the same 64-byte cache line), and the renderer is compute-bound (22 GB/s effective << 400 GB/s bandwidth ceiling).

**Supersampled ray**: march 1 reference ray, approximate 3 neighbor terrain heights via `h ≈ h_center + grad_x * Δcol + grad_y * Δrow` using the Phase 2 normal map. Would reduce gather from 4→1 per step (directly attacks the bottleneck). Not implemented — breaks at sharp discrete peaks (same class as Olperer discrete sampling problem).
