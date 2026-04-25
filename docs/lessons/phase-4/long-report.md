# Phase 4 — CPU Raymarcher: Long Report

## Part 1: The Pinhole Camera Model

### 1.1 What a Camera Is

A pinhole camera is defined by a position in 3D space and a virtual film plane. Every pixel on the output image corresponds to exactly one ray cast from the camera origin through a point on that film plane. The renderer's job is to find where each ray intersects the scene — in our case, the terrain heightmap.

### 1.2 Building an Orthonormal Basis

To generate rays, the camera needs a local coordinate system: three mutually perpendicular unit vectors — `forward`, `right`, `up` — that define the camera's orientation in world space.

Given:
- `origin`: camera position in world space
- `look_at`: the point the camera is aimed at
- `world_up`: a hint vector, always `[0, 0, 1]` (Z is up for terrain)

The derivation:

```
forward = normalize(look_at - origin)
right   = normalize(cross(forward, world_up))
up      = cross(right, forward)
```

**Why `cross(forward, world_up)` for right?**
The cross product `A × B` produces a vector perpendicular to both A and B. With `forward = [0,1,0]` (north) and `world_up = [0,0,1]`:
```
cross([0,1,0], [0,0,1]) = [1,0,0]  // east — camera right when looking north
```

**Why not use `world_up` directly as `up`?**
`world_up` is only a hint — it's not guaranteed to be perpendicular to `forward`. If the camera looks slightly upward, `forward` has a Z component, and `world_up` is not 90° from it. The two-step construction guarantees a true orthonormal basis.

**Why no normalization for `up`?**
`up = cross(right, forward)`. Both inputs are unit vectors and orthogonal, so `|cross| = 1·1·sin(90°) = 1`. Already normalized.

### 1.3 The Film Plane and FOV

The film half-extents define the frustum width at distance 1 from the origin:

```
half_w = tan(fov_horizontal / 2)
half_h = half_w / aspect_ratio
```

For `fov = 60°`: `half_w = tan(30°) ≈ 0.577`. The frustum spans ±0.577 units horizontally at distance 1.

### 1.4 Ray Generation

For pixel `(px, py)` in image of size `(W, H)`:

```
u     = (px + 0.5) / W          // center of pixel, range [0,1]
v     = (py + 0.5) / H

ndc_x = -(2*u - 1)              // [-1,1], negated to flip horizontal
ndc_y =  (1 - 2*v)              // [-1,1], flipped because screen Y goes down

cam_x = ndc_x * half_w
cam_y = ndc_y * half_h

dir   = normalize(forward + cam_x * right + cam_y * up)
```

The `+0.5` centers the ray in the pixel (not at the pixel corner). The Y flip ensures pixel (0,0) top-left maps to the upper part of the frustum. The X negation was required to match the geographic convention (east = positive X in meter space, but screen left→right corresponds to west→east when looking roughly south).

### 1.5 Coordinate System

The world coordinate system used throughout:
- **X** = meters east from tile west edge (`col × dx_meters`)
- **Y** = meters south from tile north edge (`row × dy_meters`)
- **Z** = elevation in meters

This is critical: the camera constructor, the raymarch pixel conversion, and the normal lookup must all use the same cell sizes. **Hardcoding `21.06` and `30.87` while the heightmap stores `20.691` and `30.922` caused a 41-column drift in ray positions** — discovered through careful debugging.

The fix: always read `hm.dx_meters` and `hm.dy_meters` from the heightmap struct and cast to `f32` at the call site. Never hardcode geographic constants.

### 1.6 Converting Google Earth Coordinates

Google Earth provides: latitude, longitude, camera altitude, heading (clockwise from north), tilt (90°=horizontal, 0°=straight down).

Conversion to pixel space:
```
cam_col = (lon - 11.0) * 3600
cam_row = (48.0 - lat) * 3600
```

Look-at direction from heading H and tilt T (where tilt_below_horizontal = 90 - T):
```
look_at_x = cam_x + sin(H) * cos(tilt_below) * dist
look_at_y = cam_y - cos(H) * cos(tilt_below) * dist
look_at_z = cam_z - sin(tilt_below) * dist
```

Note the negative sign on `cos(H)` for Y: compass north = decreasing row = negative Y.

---

## Part 2: Heightmap Raymarching

### 2.1 Why No Closed-Form Solution

A heightmap has no analytic intersection formula. The terrain surface is defined by discrete samples — you cannot solve "where does this line intersect this surface" algebraically. The solution is **iterative marching**: step along the ray, sample the heightmap at each position, and detect when the ray goes below the terrain.

### 2.2 The March Loop

```
t = 0
while t < t_max:
    p = origin + t * dir           // world-space position
    col = p.x / dx_m               // pixel column
    row = p.y / dy_m               // pixel row
    if out of bounds → None
    terrain_z = hm[row][col]
    if p.z < terrain_z → HIT
    t += step_m
return None
```

The parametric form `P(t) = origin + t * dir` works because `dir` is normalized — `t` is directly in meters.

### 2.3 Step Size

The step size determines accuracy and performance. At `step_m = dx_meters ≈ 20m`, the ray advances ~1 pixel per step in XY — it cannot skip a terrain feature the heightmap represents. Halving step size doubles iterations.

**The discrete sampling problem**: a ray that barely clips a narrow peak can miss it entirely if the peak occupies a single pixel. The ray passes through a lower neighbor pixel on one side and a lower neighbor on the other, never landing on the peak pixel itself. This was demonstrated concretely: Olperer's 3449m peak was stored at one pixel, the ray threaded through adjacent 3010m and 3002m pixels 41 columns away due to the coordinate bug.

### 2.4 Binary Search Refinement

Once a crossing is detected (ray was above at `t_prev`, below at `t`), the true intersection is somewhere in `[t_prev, t]`. Binary search bisects this interval 8 times:

```
lo = t - step_m, hi = t
repeat 8 times:
    mid = (lo + hi) / 2
    p = origin + mid * dir
    if p.z < terrain_z → hi = mid   // still below
    else               → lo = mid   // above
return origin + lo * dir
```

After 8 bisections with `step_m = 20m`: error = `20 / 2^8 ≈ 0.08m`. Sub-meter accuracy at negligible cost (~8 extra samples per ray, amortized over 506 average steps).

**Invariant**: `lo` always above terrain, `hi` always below. Return `lo` — the last confirmed above-ground point.

### 2.5 Coordinate Conversion Bug

The most instructive debugging episode of the phase. Symptoms:
1. Ray missed peak at expected pixel coordinates
2. Debug print showed ray hitting col=2411 when peak was at col=2370
3. 41-column drift = `2388 * (21.06 - 20.691) / 20.691`

Root cause: camera built with hardcoded `21.06 m/pixel` but raymarch divided by `hm.dx_meters = 20.691`. The 1.8% discrepancy compounded over 2388 columns to a 41-pixel error. **Lesson: use one authoritative source for all unit conversions.**

---

## Part 3: Shading

### 3.1 Lambertian Shading

At the hit point, look up the precomputed normal from Phase 2:
```
col = p.x / dx_m
row = p.y / dy_m
idx = row * cols + col

nx = normals.nx[idx]
ny = normals.ny[idx]
nz = normals.nz[idx]

diffuse = max(0, dot(normal, sun_dir))
```

`diffuse` ranges from 0 (surface facing away from sun) to 1 (surface facing directly at sun). The `max(0, ...)` clamps negative values — no "negative light."

### 3.2 Ambient + Shadow

```
shadow = shadow_mask.data[idx]   // 0.0 = in shadow, 1.0 = lit
brightness = ambient + (1.0 - ambient) * diffuse * shadow
```

- `ambient = 0.4`: minimum brightness so shadowed areas aren't black
- Shadowed pixels (`shadow=0`): brightness = 0.4 (ambient only)
- Lit, direct-facing pixels: brightness approaches 1.0

### 3.3 Elevation Color Bands

Base color selected by elevation before applying brightness:
```
< 1800m  → [120, 160, 80]   // green (vegetation)
< 2000m  → [160, 175, 130]  // transitional
< 2600m  → [200, 200, 195]  // grey rock
≥ 2600m  → [240, 245, 250]  // glacier snow white
```

### 3.4 Sun Alignment

The sun direction must be consistent between shading and shadow mask computation. The shadow mask from Phase 3 is computed from azimuth/elevation angles; the shade function uses a 3D unit vector. To align them:

```rust
let sun_dir = [0.4f32, 0.5f32, 0.7f32];
let sun_azimuth_rad   = sun_dir[0].atan2(-sun_dir[1]);  // atan2(east, north)
let sun_elevation_rad = sun_dir[2].atan2((sun_dir[0].powi(2) + sun_dir[1].powi(2)).sqrt());
```

Define `sun_dir` once, derive azimuth/elevation from it. Pass `sun_dir` to render and shading; pass azimuth/elevation to shadow mask computation. Single source of truth.

---

## Part 4: NEON Packet Raytracing

### 4.1 Motivation

Process 4 rays simultaneously using NEON `float32x4_t` registers. Each SIMD lane holds one independent ray. All 4 rays step together; lanes that have hit terrain are masked out.

### 4.2 SoA Layout

4 rays in Struct-of-Arrays layout:
```rust
pub struct RayPacket {
    origin_x: float32x4_t,  // x origins of rays 0-3
    origin_y: float32x4_t,
    origin_z: float32x4_t,
    dir_x:    float32x4_t,
    dir_y:    float32x4_t,
    dir_z:    float32x4_t,
}
```

SoA allows one `vmulq_f32` to advance all 4 rays' X positions simultaneously, vs AoS which would require interleaved access.

### 4.3 Active Mask

```rust
let mut is_active: uint32x4_t = vdupq_n_u32(u32::MAX);  // all lanes live
```

Each step:
```
hit_mask  = vcltq_f32(p_z, terrain_z)              // p_z < terrain_z
new_hits  = vandq_u32(hit_mask, is_active)         // only active lanes
is_active = vbicq_u32(is_active, new_hits)         // clear newly hit lanes
```

`vbicq_u32(a, b) = a & ~b` — clears bits in `a` where `b` is set.

Early exit: `vmaxvq_u32(is_active) == 0` when all lanes are done.

### 4.4 The Gather Problem

NEON has no gather instruction (unlike AVX2's `_mm256_i32gather_epi32`). Loading 4 heightmap values from 4 different addresses requires 4 scalar loads + manual pack:

```rust
let t0 = hm.data[row0 * cols + col0] as f32;
let t1 = hm.data[row1 * cols + col1] as f32;
let t2 = hm.data[row2 * cols + col2] as f32;
let t3 = hm.data[row3 * cols + col3] as f32;
let terrain_z = vld1q_f32([t0, t1, t2, t3].as_ptr());
```

This is the fundamental bottleneck. 4 potentially independent cache misses per step, unavoidable without a gather instruction.

### 4.5 NEON Binary Search

`binary_search_hit_neon` operates on all 4 lanes simultaneously with per-lane `t_lo`/`t_hi` as `float32x4_t`. The `vbslq_f32` (bitselect) instruction updates bounds per lane:

```
below = vcltq_f32(p_z, terrain_z)
t_hi  = vbslq_f32(below, t_mid, t_hi)  // below → shrink upper bound
t_lo  = vbslq_f32(below, t_lo, t_mid)  // above → raise lower bound
```

Called only when `vmaxvq_u32(new_hits) != 0` — avoids 8 wasted gathers on the 505 non-hit steps.

### 4.6 The Divergence Cost

When 1 of 4 lanes hits at `t=500m` and the other 3 hit at `t=1500m`, the loop must continue for 1000m/step_m extra iterations with the first lane doing wasted arithmetic. This is the CPU analog of GPU warp divergence. The binary search likewise refines all 4 lanes even when only 1 hit — the inactive lanes' results are discarded.

---

## Part 5: Parallelism with Rayon

### 5.1 Row-Level Parallelism

```rust
framebuffer
    .par_chunks_mut((width * 3) as usize)
    .enumerate()
    .for_each(|(py, row_buf)| {
        for px in 0..width {
            // ray generation + march + shade + write to row_buf
        }
    });
```

`par_chunks_mut` splits the framebuffer into non-overlapping row slices — each thread gets an independent mutable slice. No false sharing: rows are `width × 3` bytes apart (~6000 bytes >> 64-byte cache line).

Each ray is independent: reads from shared immutable `hm`, `normals`, `shadow_mask`; writes to its own row slice. No synchronization needed.

### 5.2 `par_bridge` vs `par_chunks_mut`

`par_bridge()` converts a sequential `Iterator` to a rayon `ParallelIterator`. It works but serializes iterator state internally. `par_chunks_mut` is the idiomatic choice for slices — natively parallel, no bridging overhead.

---

## Part 6: Performance Results and Hardware Analysis

### 6.1 Numbers (M4 Max, 2000×900, step_m=dx_meters)

| Version | Time | Speedup |
|---|---|---|
| Scalar single-thread | 0.80s | 1× |
| NEON single-thread | 0.80s | 1× |
| Scalar parallel (10 cores) | 0.08s | **10×** |
| NEON parallel (10 cores) | 0.08s | **10×** |

Average steps per ray: **506** (at step_m ≈ 20.7m, average ray travels ~10.5km).

### 6.2 Why NEON Doesn't Win

The workload is **memory-bound with cache-friendly access**. Each ray makes 506 sequential heightmap samples, advancing ~1 pixel per step — a predictable stride the hardware prefetcher detects and prefetches ahead. The scalar inner loop is also auto-vectorized by the compiler in release mode.

Manual NEON adds:
- Packing overhead: 6 × `vld1q_f32` with temporary stack arrays to build `RayPacket`
- 8 `vgetq_lane_f32` extracts per step (NEON register → GP register movement)
- `vdupq_n_f32(t)` broadcast every step

These costs roughly cancel the SIMD arithmetic benefit. Earlier (without the `vmaxvq_u32` guard on binary search), NEON was 17% slower. With the guard, they tie.

**The key insight**: for a single sequential ray, the hardware prefetcher trains perfectly on a ~1-pixel/step stride. NEON fires 4 simultaneous streams — still nearby, but the prefetcher trains less effectively on interleaved streams.

### 6.3 Why Parallelism Wins

10× speedup from 10 cores is near-ideal scaling. The workload is:
- Embarrassingly parallel: each pixel independent
- No shared mutable state
- Working set per thread (one row of rays × heightmap region) fits in L2

Memory bandwidth at parallel peak: `1.8M rays × 506 steps × 2 bytes / 0.08s ≈ 22 GB/s` — well below M4 Max's ~400 GB/s ceiling. Not bandwidth-limited yet, which is why scaling is near-linear.

### 6.4 The General Lesson

> **For memory-bound workloads with sequential access patterns: parallelism across cores >> manual SIMD within a core. SIMD helps arithmetic-bound code; memory-bound code needs parallelism or better access patterns (tiling, Morton order).**

This is a measured, hardware-validated conclusion, not intuition.

---

## Part 7: Module Structure

```
render_cpu/src/
├── lib.rs           — mod declarations + pub use re-exports
├── camera.rs        — Camera, Ray, ray_for_pixel
├── vector_utils.rs  — pub(crate) add, sub, scale, normalize, cross
├── raymarch.rs      — raymarch(), binary_search_hit()
├── raymarch_neon.rs — RayPacket, raymarch_neon(), binary_search_hit_neon()
├── render.rs        — render() scalar, render_par() parallel
└── render_neon.rs   — render_neon() NEON, render_neon_par() NEON+parallel
```

Key design decisions:
- `vector_utils.rs` as `pub(crate)` — shared math helpers, not part of public API
- `binary_search_hit` as private function — separation of bracket detection from refinement
- `shade` as `pub(crate)` — shared between `render.rs` and `render_neon.rs`
- `RayPacket::new` takes 4 `&Ray` — caller constructs scalar rays first, then packs

---

## Part 8: Gotchas and Errors Encountered

1. **Hardcoded cell sizes**: using `21.06` and `30.87` instead of `hm.dx_meters` caused a 41-column drift — the single most impactful bug of the phase. Always use authoritative sources for unit conversions.

2. **`vgetq_lane_f32` requires const lane index**: cannot loop over lanes with a variable index. Must unroll 4 explicit calls. Rust NEON bindings enforce this at compile time.

3. **`vcltq_f32` returns `uint32x4_t` on AArch64**: no `vreinterpretq_u32_f32` needed. On some platforms the return type differs — check the signature before adding reinterpret casts.

4. **`add` typo in `vector_utils.rs`**: `[a[0]+b[0], a[1]+b[0], a[2]+b[0]]` — Y and Z used `b[0]` instead of `b[1]`, `b[2]`. All vector arithmetic should be checked component by component.

5. **`Camera::new` typo**: originally wrote `vaddq_f32(dir, t_vec)` instead of `vmulq_f32(dir, t_vec)` — adding t to direction instead of multiplying. Caught during correctness verification.

6. **Binary search called unconditionally**: calling `binary_search_hit_neon` outside the `vmaxvq_u32(new_hits) != 0` guard wasted 8 gathers × 505 non-hit steps per ray. Guard saves ~4000 wasted memory operations per ray.

7. **`par_bridge()` vs `par_chunks_mut`**: `par_bridge` works but is less efficient for slices. Use `par_chunks_mut` for idiomatic rayon parallelism over contiguous buffers.
