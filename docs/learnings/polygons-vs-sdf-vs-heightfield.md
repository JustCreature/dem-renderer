# Polygons vs SDFs vs Heightfield Raymarching

Two fundamentally different answers to the same problem: *"given a 3D scene and a
camera, what color is each pixel?"* The split is **where the geometry lives** and
**which direction you traverse** the projection.

This renderer uses the third option — heightfield raymarching — which borrows the
*structure* of SDF raymarching but not its distance guarantee. See the bottom
section for how that maps onto `crates/render_gpu/src/shader_texture.wgsl`.

---

## Polygons (rasterization)

**Representation:** the surface is an explicit mesh — a list of vertices (x,y,z)
stitched into triangles. A sphere isn't "a sphere," it's 5,000 triangles
approximating one. The geometry is *enumerated*: you store every piece of surface
up front.

**Algorithm — you loop over geometry, not pixels:**

1. For each triangle, transform its 3 vertices by the model→view→projection
   matrices. This lands each vertex in 2D screen space (plus a depth).
2. **Rasterize:** figure out which pixels the triangle covers. Hardware does an
   edge-function test per pixel — "is this pixel inside the 3 edges?"
3. For each covered pixel, run a **fragment shader** to compute color (texture
   lookup, lighting).
4. **Z-buffer:** each pixel stores the depth of the closest triangle seen so far.
   A new fragment writes only if it's nearer. This resolves occlusion without
   sorting.

Mental model: rasterization is **scatter** — geometry is thrown *at* the screen,
and each triangle "lights up" the pixels it lands on. You iterate triangles, and
pixels are a side effect.

**Why hardware loves it:** the whole pipeline is fixed-function and massively
parallel per-triangle and per-pixel. Cost scales with `triangles × pixels_covered`.
A triangle that covers 0 pixels (off-screen, back-facing) is cheap to reject. This
is what GPUs were *built* for — decades of silicon dedicated to exactly this loop.

**Cost characteristics:**
- More detail = more triangles = more vertex work + memory. A film-quality asset
  can be millions of triangles.
- Smooth/curved surfaces are *faked* — tessellate finer until the facets are
  sub-pixel.
- Hard things: true reflections, refraction, soft shadows, global illumination —
  rasterization only knows "this triangle covers this pixel," with no cheap way to
  ask "what's along this arbitrary ray?" You bolt those on with tricks (shadow
  maps, screen-space reflections) or move to ray tracing.

---

## SDFs (signed distance fields, via raymarching)

**Representation:** the surface is *implicit* — defined by a function `f(p)` that
returns, for any point `p` in space, the **distance to the nearest surface**.
Signed: negative inside the object, positive outside, zero *on* the surface. A
sphere of radius `r` at origin is literally:

```
f(p) = length(p) - r
```

That one line *is* the sphere — exactly, at infinite resolution, no triangles. You
don't store geometry; you store a *rule* for measuring distance to it.

**Algorithm — you loop over pixels, not geometry (sphere tracing):**

1. For each pixel, shoot a ray from the camera through it.
2. Evaluate `f(p)` at the current position. It returns `d` — distance to the
   nearest surface *in any direction*.
3. That `d` is a **safety radius**: nothing is closer than `d`, so you can leap
   forward exactly `d` along the ray without risk of skipping through anything.
4. Repeat. As you approach a surface, `d → 0` and steps shrink automatically. When
   `d < epsilon`, you've hit.

Mental model: raymarching is **gather** — each pixel asks "what's the first thing
along my ray?" You iterate pixels, and geometry is queried. The genius of the SDF
is that the distance value *tells you how far you're allowed to jump*, so you take
big steps in empty space and tiny careful steps near surfaces. That's why it's
called *sphere tracing* — at each point an empty sphere of radius `d` is safe to
cross.

**Why it's powerful:**
- **Infinite precision** — no facets, curves are exact.
- **Cheap CSG** — union is `min(f1, f2)`, intersection is `max`, subtraction is
  `max(f1, -f2)`. Blending/morphing is a smooth `min`. This is why Shadertoy demos
  build whole worlds in a few lines.
- **Free side effects** — because you can evaluate `f(p)` anywhere, you get soft
  shadows, AO, and reflections nearly for free (march another ray; AO ≈ sample `f`
  at a few points along the normal).
- **Surface normal** for lighting is just the gradient of `f` (finite
  differences) — no stored normals needed.

**Cost characteristics:**
- Cost scales with `pixels × steps_per_ray × cost_of_f`. Complex scenes mean
  evaluating `f` (a `min` over many primitives) dozens of times per pixel per ray.
- Grazing angles and thin/high-frequency features are the enemy — the ray creeps
  in tiny steps, or overshoots and misses.
- Doesn't map to fixed-function hardware — all general compute shader work, no
  dedicated silicon.

---

## The core differences, side by side

| | Polygons (raster) | SDF (raymarch) |
|---|---|---|
| Geometry | Explicit (stored triangles) | Implicit (a function) |
| Traversal | Scatter: loop geometry → pixels | Gather: loop pixels → query geometry |
| Resolution | Faceted, finite | Exact, infinite |
| Occlusion | Z-buffer | First hit along ray |
| Cost driver | triangle count × coverage | steps per ray × `f` complexity |
| Hardware | Dedicated fixed-function pipeline | General compute |
| Shadows/reflections/AO | Bolted on with tricks | Nearly free (march more rays) |
| Sweet spot | Detailed authored assets, games | Procedural/CSG scenes, smooth blends |
| Pain point | Curves, secondary rays | Thin features, grazing rays, `f` cost |

---

## Where this renderer sits: heightfield raymarching

We do the **gather** style (loop pixels, march rays) like SDFs — but **not** with a
distance function. Our `f(p)` doesn't return distance-to-nearest-surface; it
returns terrain height at an (x,y) and we test `ray.z < height`. That's a
**heightfield raymarch**.

The difference matters precisely at the step-size problem:

- A true SDF gives you a *guaranteed safe jump distance in all 3D directions*.
- Our heightfield only knows **vertical clearance** (`pos.z - h`) — how far the ray
  is *above* the terrain straight down. That's a weaker guarantee: if terrain rises
  faster horizontally than the step moves us, vertical clearance lies and the ray
  punches through a ridge.

In `shader_texture.wgsl`:
- Adaptive step (line ~586): `t += max((pos.z - h) * sphere_factor, lod_min_step);`
  — step scales by vertical clearance ("sphere tracing" comment, 0.5 safety
  factor). `sphere_factor` is quality-controlled (Ultra 0.1 → Low 0.8). Bigger
  factor = more overshoot = thin ridges disappear on Low.
- `lod_min_step` is the floor that prevents the ray stalling near the surface.
- Once the march detects it crossed below the surface, `binary_search_hit(t_prev,
  t, dir, 10)` (line ~573) refines the hit with **10 iterations** of bisection
  between `t_prev` (last point above) and `t` (first below).

So we get the best-fit structure for terrain — a heightfield query is *far* cheaper
than a general SDF (one texture sample vs. a `min` over primitives) — at the cost
of that weaker stepping guarantee, patched with the minimum-step floor and the
binary-search refinement.

The "arc" artefacts in the foreground are the step-size error made visible: all
rays with the same step count share the same overshoot distance, creating
concentric iso-step contours.

---

## The third giant: ray-traced polygons (RTX)

Same "gather" traversal as raymarching, but instead of stepping along the ray
evaluating a function, it intersects the ray analytically against triangles using a
BVH acceleration structure. It's the convergence point: explicit geometry like
rasterization, ray-per-pixel traversal like raymarching — and the part modern GPUs
now accelerate in dedicated hardware.
