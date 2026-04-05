# Phase 4 — CPU Raymarcher: Short Report

## 1. Pinhole Camera Model

**Orthonormal basis construction** from `origin`, `look_at`, `world_up = [0,0,1]`:
```
forward = normalize(look_at - origin)
right   = normalize(cross(forward, world_up))
up      = cross(right, forward)          // no normalization needed
```
`up` requires no normalization — `right` and `forward` are unit and orthogonal → `|cross| = 1`.

**Film half-extents**:
```
half_w = tan(fov_deg / 2 * π/180)
half_h = half_w / aspect_ratio
```

**Ray generation** for pixel `(px, py)` in `(W, H)` image:
```
u     = (px + 0.5) / W
v     = (py + 0.5) / H
ndc_x = -(2u - 1)          // negated to flip horizontal (geographic convention)
ndc_y =  (1 - 2v)          // flipped: screen Y down, camera Y up
dir   = normalize(forward + ndc_x*half_w*right + ndc_y*half_h*up)
```

**World coordinate system**: X = meters east, Y = meters south, Z = elevation meters.
Camera at `[col * dx_m, row * dy_m, z]`. Must use `hm.dx_meters`/`hm.dy_meters` everywhere — never hardcode cell sizes.

**Google Earth conversion** (heading H°, tilt T° where 90°=horizontal):
```
col = (lon - 11.0) * 3600
row = (48.0 - lat) * 3600
tilt_below = (90 - T) * π/180
look_at_x = cam_x + sin(H°) * cos(tilt_below) * dist
look_at_y = cam_y - cos(H°) * cos(tilt_below) * dist
look_at_z = cam_z - sin(tilt_below) * dist
```

---

## 2. Heightmap Raymarching

**Core loop**:
```
t = 0
while t < t_max:
    p = origin + t * dir
    col = p.x / dx_m,  row = p.y / dy_m
    if OOB → return None
    if p.z < hm[row][col] → binary_search(t - step_m, t) → return Some(hit)
    t += step_m
```

**Step size**: `step_m = dx_meters` (≈20.7m) = 1 pixel per step. Cannot miss a representable feature. Average steps per ray: **506** (≈10.5 km travel).

**Binary search refinement**:
```
lo = t - step_m,  hi = t    // lo=above, hi=below — guaranteed bracket
repeat 8 times:
    mid = (lo + hi) / 2
    if p(mid).z < terrain → hi = mid
    else                  → lo = mid
return p(lo)                // last confirmed above-ground point
```
Accuracy: `20m / 2^8 ≈ 0.08m`. Cost: 8 extra heightmap samples per ray (negligible vs 506 average steps).

**The discrete sampling problem**: a single-pixel-wide peak can be missed if the ray threads through lower neighbor pixels. The ray's fractional XY position may never land on the peak pixel. Manifested as Olperer (3449m peak) being missed — the ray passed through col=2369 (terrain=3010m) instead of col=2370 (3449m).

**Coordinate bug**: hardcoded `21.06 m/pixel` while `hm.dx_meters = 20.691`. 1.8% error × 2388 columns = **41-column drift**. Fix: always read from `hm.dx_meters as f32`.

---

## 3. Shading

**Lambertian shading**:
```
idx     = row * cols + col
diffuse = max(0, dot([nx[idx], ny[idx], nz[idx]], sun_dir))
shadow  = shadow_mask.data[idx]      // 0=shadow, 1=lit
brightness = ambient + (1 - ambient) * diffuse * shadow
color = base_color * brightness
```

**Elevation color bands**:
```
< 1800m → [120,160,80]    green
< 2000m → [160,175,130]   transitional
< 2600m → [200,200,195]   grey rock
≥ 2600m → [240,245,250]   glacier white
```

**Sun alignment**: define `sun_dir: [f32; 3]` once, derive both:
```rust
let azimuth   = sun_dir[0].atan2(-sun_dir[1]);   // atan2(east, north)
let elevation = sun_dir[2].atan2(hypot(sun_dir[0], sun_dir[1]));
```
Pass `sun_dir` to shade; pass `azimuth`/`elevation` to shadow mask. Single source of truth.

**Shading vs shadowing**:
- Shading: slope facing away from sun → dark (local, from normal)
- Shadowing: another mountain blocks sun → dark (global, from DDA sweep)
- Both stack: `brightness = ambient + (1-ambient) * diffuse * shadow`

---

## 4. NEON Packet Raytracing

**RayPacket — SoA layout**:
```rust
struct RayPacket {
    origin_x/y/z: float32x4_t,   // 4 ray origins packed by component
    dir_x/y/z:    float32x4_t,   // 4 ray directions packed by component
}
```
SoA allows `vmulq_f32(dir_x, t_vec)` to advance all 4 X components in one instruction.

**Active mask**:
```rust
let mut is_active: uint32x4_t = vdupq_n_u32(u32::MAX);
// per step:
let hit_mask  = vcltq_f32(p_z, terrain_z);
let new_hits  = vandq_u32(hit_mask, is_active);
is_active     = vbicq_u32(is_active, new_hits);   // vbic = a & ~b
if vmaxvq_u32(is_active) == 0 { return result; }
```

**Gather** (no NEON gather instruction):
```rust
let t0 = hm.data[rows[0]*cols + cols[0]] as f32;  // 4 scalar loads
// ... t1, t2, t3
let terrain_z = vld1q_f32([t0,t1,t2,t3].as_ptr());
```
Requires extracting float lanes: `vgetq_lane_f32(col_f, 0)` — lane index must be a compile-time const.

**OOB handling**: use `c.clamp(0, cols-1)` before gather to avoid panic, then deactivate OOB lanes via the `oob` mask.

**NEON binary search**: per-lane `t_lo`/`t_hi` as `float32x4_t`. `vbslq_f32(mask, a, b)` (bitselect) updates bounds per lane independently. Only called when `vmaxvq_u32(new_hits) != 0` — avoids 8 wasted gathers × 505 non-hit steps.

**Divergence**: when lanes hit at different `t`, all lanes continue marching until the last one hits. Dead lanes do wasted arithmetic but results are masked. CPU analog of GPU warp divergence.

---

## 5. Rayon Parallelism

**Pattern**:
```rust
framebuffer
    .par_chunks_mut((width * 3) as usize)   // one chunk per row
    .enumerate()
    .for_each(|(py, row_buf)| {
        for px in 0..width {
            let idx = (px * 3) as usize;    // py*width already baked in by chunking
            row_buf[idx..idx+3].copy_from_slice(&shade(...));
        }
    });
```

**Why no false sharing**: rows are `width × 3 ≈ 6000` bytes apart >> 64-byte cache line. Each thread writes to a disjoint memory region.

**`par_chunks_mut` vs `par_bridge`**: `par_chunks_mut` is natively parallel for slices. `par_bridge` converts sequential iterator to parallel — slightly more overhead, avoid for slices.

---

## 6. Performance Results

**Hardware**: Apple M4 Max, 10 cores, 2000×900 image, `step_m = dx_meters ≈ 20.7m`

| Version | Time | vs scalar |
|---|---|---|
| Scalar single-thread | 0.80s | 1× |
| NEON single-thread | 0.80s | 1× |
| Scalar parallel (10 cores) | **0.08s** | **10×** |
| NEON parallel (10 cores) | **0.08s** | **10×** |

Average steps per ray: **506**

**Why NEON ≈ scalar**:
- Workload is memory-bound with sequential access (prefetcher-friendly stride ~1 pixel/step)
- Compiler auto-vectorizes scalar `raymarch` in release mode
- Manual NEON adds packing overhead (`RayPacket::new`: 6 stack arrays + 6 `vld1q_f32`), 8 lane extracts/step, broadcast `t` each step
- These costs cancel the SIMD arithmetic benefit

**Why parallel wins**: embarrassingly parallel workload, no shared mutable state, independent rows. 10× speedup from 10 cores = near-ideal scaling. Not bandwidth-limited: `1.8M rays × 506 steps × 2B / 0.08s ≈ 22 GB/s` << M4 Max's 400 GB/s.

**Key lesson**: for memory-bound code with sequential access, parallelism across cores >> manual SIMD. SIMD helps arithmetic-bound code. Measure first, optimize the actual bottleneck.

---

## 7. Module Structure

```
render_cpu/src/
├── lib.rs           — mod + pub use re-exports
├── camera.rs        — Camera, Ray
├── vector_utils.rs  — pub(crate): add, sub, scale, normalize, cross
├── raymarch.rs      — raymarch(), binary_search_hit() [private]
├── raymarch_neon.rs — RayPacket, raymarch_neon(), binary_search_hit_neon() [private]
├── render.rs        — render() [scalar], render_par() [rayon]
└── render_neon.rs   — render_neon() [NEON], render_neon_par() [NEON+rayon]
```

`shade()` is `pub(crate)` — shared between `render.rs` and `render_neon.rs`.

---

## 8. Key Gotchas

| Gotcha | Fix |
|---|---|
| Hardcoded cell sizes → 41-column drift | Always use `hm.dx_meters as f32` |
| `vgetq_lane_f32` needs const index | Unroll 4 explicit calls |
| `vcltq_f32` returns `uint32x4_t` (no reinterpret needed) | Check signature, don't add spurious cast |
| `add()` typo: all components used `b[0]` | Check every component explicitly |
| Binary search called unconditionally | Guard with `vmaxvq_u32(new_hits) != 0` |
| `par_bridge` on slice | Use `par_chunks_mut` instead |
| `row_buf[idx]` vs `framebuffer[idx]` in closure | Closure captures `row_buf` not `framebuffer` |
