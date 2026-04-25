# Session Log: Normals, Lighting & Geometry (Phase 2 Additional)

Date: 2026-03-29

---

**Q: Those normals we built in phase 2 — does the normal map show shadows with the sun directly above? What is the meaning of the normal map and the PNG?**

No, normals and shadows are completely separate things. A normal map encodes surface orientation — each pixel stores the unit vector perpendicular to the terrain surface at that point. It has no sun, no light source, no shadows. It is purely geometry — "which direction is this patch of terrain facing?"

The PNG is an input to shading, not a shading result. In Phase 3/4 you combine normals with a sun direction vector to get `brightness = dot(normal, sun_direction)`. That is when you get actual lighting. Phase 3 adds horizon shadows on top of that.

---

**Q: But it's black and white, no colors. Explain what I see as different shades of gray.**

The PNG saves only the `nz` component (z-component of the normal) as a `GrayImage`:

```rust
let pixels: Vec<u8> = normal_map.nz.iter().map(|&v| (v * 255.0) as u8).collect();
image::GrayImage::from_raw(heightmap.cols as u32, heightmap.rows as u32, pixels)
    .unwrap()
    .save("artifacts/normals_nz.png")
    .unwrap();
```

`nz` is how much the surface faces straight up (vertical component). Flat terrain → `nz = 1.0` → white. Sloped terrain → `nz < 1.0` → gray. Steep cliff → `nz ≈ 0.0` → black. It is a slope steepness map without directional information.

---

**Q: Show me code how to write the RGB image.**

User had created an empty `fn create_rgb_png() {}`. Filled it:

```rust
fn create_rgb_png(heightmap: &Heightmap, normal_map: &NormalMap) {
    let pixels: Vec<u8> = (0..heightmap.rows * heightmap.cols)
        .flat_map(|i| {
            let nx = ((normal_map.nx[i] + 1.0) * 0.5 * 255.0) as u8;
            let ny = ((normal_map.ny[i] + 1.0) * 0.5 * 255.0) as u8;
            let nz = (normal_map.nz[i] * 255.0) as u8;
            [nx, ny, nz]
        })
        .collect();

    image::RgbImage::from_raw(heightmap.cols as u32, heightmap.rows as u32, pixels)
        .unwrap()
        .save("artifacts/normals_rgb.png")
        .unwrap();
}
```

`nx`/`ny` ∈ `[-1.0, 1.0]` need remapping to `[0, 255]` via `(v + 1.0) * 0.5`. `nz` ∈ `[0.0, 1.0]` always for terrain, no remapping needed.

---

**Q: Explain `((normal_map.nx[i] + 1.0) * 0.5 * 255.0)`. Explain the colors.**

`nx` ∈ `[-1.0, 1.0]` but `u8` can't store negatives. `(nx + 1.0) * 0.5` maps to `[0.0, 1.0]`:

| nx | →u8 | meaning |
|---|---|---|
| -1.0 | 0 (black) | max west-facing slope |
| 0.0 | 128 (mid gray) | no east/west tilt |
| +1.0 | 255 (white) | max east-facing slope |

Color meaning: red = nx (east/west tilt), green = ny (north/south tilt), blue = nz (upward component). Flat terrain → `(128, 128, 255)` — the classic blue-purple of game normal maps.

---

**Q: Explain how we built those normals. How do we calculate north/south or east/west vectors?**

The method is **finite differences** — estimate slope from two neighbours on each axis:

```rust
let upper = hm.data[(r - 1) * hm.cols + c];
let lower = hm.data[(r + 1) * hm.cols + c];
let left  = hm.data[r * hm.cols + (c - 1)];
let right = hm.data[r * hm.cols + (c + 1)];

let single_nx = (left - right) / (2.0 * hm.dx_meters);
let single_ny = (upper - lower) / (2.0 * hm.dy_meters);
let single_nz = 1.0;
```

Terrain is a height function `h(x, y)`. The unnormalized normal to a height field is `(-dh/dx, -dh/dy, 1)`. Central difference: `dh/dx ≈ (right - left) / (2 * dx_meters)`. So `nx = left - right` is the negative eastward slope. `nz = 1.0` hardcoded because for any height field `z = h(x,y)` the z-component is always 1 before normalization — it comes from the math. Division by `dx_meters`/`dy_meters` corrects for DEM pixels not being square in meters.

---

**Q: Explain why we calculate length like `sqrt(nx² + ny² + nz²)`. Explain what dot product means.**

**Length:** Pythagoras in 3D. In 2D: `sqrt(3² + 4²) = 5`. In 3D: `sqrt(x² + y² + z²)`. A vector `(nx, ny, nz)` is an arrow; its length is the straight-line distance from origin to tip.

**Normalization:** Dividing by length scales to unit length without changing direction: `sqrt((nx/L)² + (ny/L)² + (nz/L)²) = sqrt(L²/L²) = 1`.

**Dot product:**
```
A · B = Ax*Bx + Ay*By + Az*Bz  =  |A| * |B| * cos(θ)
```
For unit vectors: `A · B = cos(θ)` — the cosine of the angle between them.

For lighting: `brightness = dot(normal, sun_direction)`. Faces sun directly → 0° → 1.0. Perpendicular to sun → 90° → 0.0. Faces away → negative → clamp to 0. This is Lambert's cosine law — physically correct.

---

**Q: What if we have 2 light sources of different power?**

Add contributions independently — light is additive energy:

```
brightness = dot(normal, sun1_dir) * sun1_power
           + dot(normal, sun2_dir) * sun2_power
```

Clamp each dot to zero before multiplying (a light behind the surface contributes nothing). Two light sources = two independent streams of photons that don't interact.

---

**Q: Explain those complex lighting models briefly.**

**Specular (Blinn-Phong):** Adds shiny reflections to matte diffuse. Compute halfway vector between light dir and view dir, then `specular = dot(normal, halfway) ^ shininess`. High exponent = tight sharp glint.

**Area lights:** Real light sources have area (sun disk, window). Approximated by sampling many point lights across the area and averaging. Produces soft shadows (penumbra) because near the boundary part of the light is visible and part isn't.

**Global illumination:** Local models only see direct light. GI adds indirect bounced light. Governed by the rendering equation (Kajiya 1986):
```
outgoing = emitted + ∫ incoming(dir) * surface_response * cos(θ) dω
```
Gives color bleeding, ambient occlusion, caustics. Solved numerically by path tracers via Monte Carlo sampling.

---

**Q: Explain the meaning of `cos()` in geometry. Explain the meaning of the integral in global illumination.**

**cos:** Draw a unit circle. At angle θ, the point on the circle is `(cos θ, sin θ)`. cos is the horizontal coordinate. It measures how much two directions agree. Solar panel facing sun: cos(0°) = 1, full power. Tilted 45°: cos(45°) = 0.7. Edge-on: cos(90°) = 0. This is why `dot(normal, light) = cos(θ)` gives correct brightness — it measures what fraction of area is "visible" to incoming light.

**Integral:** An integral sums infinitely many infinitely small contributions. In 1D, thin rectangles of width `dx`. In the rendering equation, the sum is over directions in a hemisphere — `dω` is an infinitesimally small cone of directions. For every direction light could arrive from, you add: `incoming_light × surface_response × cos(θ)`. Can't solve analytically because `incoming_light(direction)` requires tracing a ray to see what's there — recursive. Path tracers solve numerically: shoot random rays, each is one sample, average all. More rays = more accurate. This is Monte Carlo integration.

---

**Q: Explain the geometric meaning of `sin()`.**

Same unit circle. At angle θ: point = `(cos θ, sin θ)`. cos is horizontal, sin is vertical. sin measures the **perpendicular component** — how much sticks out sideways.

`sin(θ) = cos(90° - θ)` — same function shifted 90°. When cos is max, sin is zero and vice versa.

Pythagorean identity: `sin²(θ) + cos²(θ) = 1` — just Pythagoras on the unit circle.

In rendering: hemisphere integration needs `sin(θ) dθ dφ` because patches near the equator are larger than near the pole. Snell's law: `n1 * sin(θ1) = n2 * sin(θ2)`.

---

**Q: Explain tan and ctg, and how to pronounce them all.**

**Pronunciation:**
- sin → "sine" (rhymes with "mine")
- cos → "coss" (rhymes with "boss")
- tan → "tan" (like the color)
- cot → "cot" (like a small bed)

(`tg`/`ctg` is Eastern European notation — Russian/Czech/Slovak textbooks. English writes `tan`/`cot`.)

**tan:** `tan(θ) = sin(θ) / cos(θ)`. Measures slope — rise per unit horizontal. 45° → tan = 1. 80° cliff → tan ≈ 5.7. 90° wall → tan = ∞.

**cot:** `cot(θ) = cos(θ) / sin(θ) = 1 / tan(θ)`. Horizontal per unit rise. Reciprocal of tan.

Summary:

| function | measures | bounded? |
|---|---|---|
| sin | vertical / perpendicular component | yes, [-1, 1] |
| cos | horizontal / parallel component | yes, [-1, 1] |
| tan | slope (rise/run) | no, blows up at 90° |
| cot | inverse slope (run/rise) | no, blows up at 0° |

Key intuition: **cos = parallel agreement, sin = perpendicular component.**
