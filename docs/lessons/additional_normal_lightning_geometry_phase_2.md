# Additional Learnings: Normals, Lighting, and Geometry (Phase 2)

---

## 1. What Is a Normal Map?

A normal map encodes **surface orientation**, not lighting. Each pixel stores a unit vector perpendicular to the terrain surface at that point. It has no sun, no shadows — it is purely geometry.

The three components map to directions:
- `nx` — how much the surface tilts east/west
- `ny` — how much the surface tilts north/south
- `nz` — how much the surface faces straight up

Normal maps are an **input** to shading. You combine them with a sun direction in Phase 3/4 to produce actual lighting.

---

## 2. The Grayscale PNG (nz only)

The Phase 2 output `normals_nz.png` saves only the `nz` component as a grayscale image:

```rust
let pixels: Vec<u8> = normal_map.nz.iter().map(|&v| (v * 255.0) as u8).collect();
image::GrayImage::from_raw(heightmap.cols as u32, heightmap.rows as u32, pixels)
    .unwrap()
    .save("artifacts/normals_nz.png")
    .unwrap();
```

What you see:
- **White** = `nz ≈ 1.0` = flat terrain (faces straight up)
- **Gray** = moderate slopes
- **Black** = `nz ≈ 0.0` = steep cliffs and ridgelines

It is a **slope steepness map** — no directional information, only how steep each point is.

---

## 3. The RGB Normal Map

To encode all three components including slope direction:

```rust
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
```

**Why the remapping `(v + 1.0) * 0.5`:**

`nx` and `ny` ∈ `[-1.0, 1.0]` but a pixel channel is `u8` ∈ `[0, 255]` — can't store negatives. The remap shifts and compresses to `[0.0, 1.0]`:

| nx value | after `+1.0` | after `×0.5` | ×255 → u8 |
|---|---|---|---|
| -1.0 (max west tilt) | 0.0 | 0.0 | 0 (black) |
| 0.0 (no tilt) | 1.0 | 0.5 | 128 (mid gray) |
| +1.0 (max east tilt) | 2.0 | 1.0 | 255 (white) |

`nz` ∈ `[0.0, 1.0]` always for terrain (normals always point upward), so no remapping needed.

**Color meaning:**
- **Red channel** = nx → east-facing slope is bright red, west-facing is dark
- **Green channel** = ny → north-facing is bright green, south-facing is dark
- **Blue channel** = nz → flat terrain is bright blue, steep is dark

Flat terrain → `(128, 128, 255)` — the characteristic blue-purple of all game normal maps.

---

## 4. How Normals Are Computed (Finite Differences)

The core idea: estimate slope by looking at the two neighbours on each axis.

```rust
let upper = hm.data[(r - 1) * hm.cols + c];  // row above (north)
let lower = hm.data[(r + 1) * hm.cols + c];  // row below (south)
let left  = hm.data[r * hm.cols + (c - 1)]; // column left (west)
let right = hm.data[r * hm.cols + (c + 1)]; // column right (east)

let single_nx = (left - right) / (2.0 * hm.dx_meters);
let single_ny = (upper - lower) / (2.0 * hm.dy_meters);
let single_nz = 1.0;
```

**The mathematical foundation:**

Terrain is a height function `h(x, y)`. The unnormalized normal to such a surface is exactly:

```
(-dh/dx,  -dh/dy,  1)
```

`dh/dx` (slope eastward) is estimated with a central difference:

```
dh/dx ≈ (right - left) / (2 * dx_meters)
```

So `nx = left - right` is just `-(right - left)`, the negative eastward slope. If terrain rises to the right, the surface tilts east, so the normal tilts west (negative nx).

**Why `nz = 1.0` hardcoded:**

For a height field `z = h(x, y)`, the z-component of the gradient is always 1 before normalization. It comes out of the math, not an approximation.

**Why divide by `dx_meters` / `dy_meters`:**

DEM pixels are not square in meters — latitude spacing ≠ longitude spacing. Without this correction the normals would be geometrically wrong.

---

## 5. Vector Length — Why `sqrt(nx² + ny² + nz²)`

This is Pythagoras extended to 3D.

In 2D: walk 3 east, 4 north → straight-line distance = `sqrt(3² + 4²) = 5`.

In 3D: same idea applied twice:
```
length = sqrt(x² + y² + z²)
```

A vector `(nx, ny, nz)` is an arrow in 3D space. Its length is the straight-line distance from origin `(0,0,0)` to tip `(nx, ny, nz)`.

**Normalization** — dividing by length scales the arrow to exactly length 1 without changing direction:

```
length of (nx/L, ny/L, nz/L) = sqrt((nx² + ny² + nz²) / L²) = sqrt(L²/L²) = 1
```

We need unit vectors because the dot product only gives clean geometric meaning when vectors are normalized.

---

## 6. Dot Product

**Mechanical definition:**
```
A · B = Ax*Bx + Ay*By + Az*Bz
```

**Geometric meaning:**
```
A · B = |A| * |B| * cos(θ)
```

When both vectors are unit length this simplifies to just `cos(θ)` — the cosine of the angle between them.

**Why this gives correct lighting:**
```
brightness = dot(surface_normal, sun_direction)
```

| angle | dot product | meaning |
|---|---|---|
| 0° | 1.0 | surface faces directly toward sun — fully lit |
| 90° | 0.0 | surface perpendicular to sun — no light |
| >90° | negative → clamp to 0 | surface faces away — in shadow |

This is physically correct: a tilted surface intercepts less energy per unit area. It is called **Lambert's cosine law**.

---

## 7. Multiple Light Sources

Just add the contributions — lights don't interact:

```
brightness = dot(normal, sun1_dir) * sun1_power
           + dot(normal, sun2_dir) * sun2_power
```

Each light contributes independently. Clamp each dot product to zero before multiplying (a light behind the surface contributes nothing). This is physically correct because light is additive energy.

---

## 8. Complex Lighting Models

### Specular Highlights (Blinn-Phong)

Diffuse (what we have) scatters light equally in all directions — matte surfaces like dirt. Specular adds shiny reflections — polished metal, water, glass.

```
halfway_vector = normalize(light_dir + view_dir)
specular = dot(normal, halfway_vector) ^ shininess
```

The exponent controls tightness: low = broad soft highlight, high = tight sharp glint.

### Area Lights

A point light is a mathematical fiction. Real light sources have area (sun disk, window, fluorescent panel). Approximated by sampling many point lights across the area and averaging. Produces **soft shadows** — a penumbra gradient rather than a hard edge, because near the shadow boundary part of the light source is visible and part isn't.

### Global Illumination

Local models only see direct light. Global illumination adds *indirect* light — light that bounced off other surfaces first. Governed by the **rendering equation** (Kajiya, 1986):

```
outgoing_light = emitted_light + ∫ incoming_light(direction) * surface_response * cos(θ) dω
```

This gives:
- **Color bleeding** — a red wall tints nearby white surfaces pink
- **Ambient occlusion** — corners and crevices are darker because fewer rays reach them
- **Caustics** — focused light patterns from glass or water

The integral cannot be solved analytically because `incoming_light(direction)` depends on what's in the scene — recursive by nature. **Path tracers** solve it numerically via Monte Carlo: shoot hundreds of random rays, each gives one sample of the integral, average them all. More rays = more accurate estimate.

---

## 9. Trigonometry — Geometric Meaning

### The Unit Circle

All four functions come from a circle of radius 1. At angle θ from the horizontal axis, the point on the circle is:

```
(cos(θ), sin(θ))
```

### cos (cosine) — pronounced "coss"

The **horizontal coordinate** on the unit circle. Measures **how much two directions agree**.

| angle | cos | meaning |
|---|---|---|
| 0° | 1.0 | fully aligned |
| 45° | 0.707 | partially aligned |
| 90° | 0.0 | perpendicular — no agreement |
| 180° | -1.0 | opposite directions |

In rendering: `dot(normal, light) = cos(θ)` — gives the fraction of energy a tilted surface receives.

### sin (sine) — pronounced "sine" (rhymes with "mine")

The **vertical coordinate** on the unit circle. Measures **the perpendicular component** — how much sticks out sideways relative to a direction.

```
sin(θ) = cos(90° - θ)
```

sin and cos are the same function shifted by 90°. When one is maximum, the other is zero.

**Pythagorean identity:**
```
sin²(θ) + cos²(θ) = 1
```

Just Pythagoras on the unit circle — the point `(cos θ, sin θ)` is always at distance 1 from the origin.

**Where sin appears in rendering:**
- Hemisphere integration: patch area near the equator is larger than near the pole — `sin(θ)` accounts for that stretching in `sin(θ) dθ dφ`
- Snell's law (refraction): `n1 * sin(θ1) = n2 * sin(θ2)` — bending of light at material boundaries

### tan (tangent) — pronounced "tan"

```
tan(θ) = sin(θ) / cos(θ)
```

Measures **slope** — rise per unit of horizontal distance.

- 45° slope → tan(45°) = 1 → rise equals run
- 80° cliff → tan(80°) ≈ 5.7 → rises 5.7m per 1m horizontal
- 90° wall → tan(90°) = ∞ → infinite slope

Notation: English writes `tan`, Eastern European textbooks (Russian/Czech/Slovak) write `tg`.

### cot (cotangent) — pronounced "cot"

```
cot(θ) = cos(θ) / sin(θ) = 1 / tan(θ)
```

The reciprocal of tan. Where tan asks "rise per unit horizontal", cot asks "horizontal per unit rise". Notation: English writes `cot`, Eastern European textbooks write `ctg`.

### Summary Table

| function | pronunciation | measures | max at | zero at | blows up at |
|---|---|---|---|---|---|
| sin | "sine" | vertical / perpendicular component | 90° | 0° | never |
| cos | "coss" | horizontal / parallel component | 0° | 90° | never |
| tan | "tan" | slope (rise/run) | — | 0° | 90° |
| cot | "cot" | inverse slope (run/rise) | — | 90° | 0° |

sin and cos are bounded `[-1, 1]`. tan and cot are unbounded — they appear less in rendering because quantities that blow up to infinity are hard to work with.

**The key intuition to hold:**
> cos = parallel agreement, sin = perpendicular component.
> Use cos when projecting *along* a direction. Use sin when measuring what sticks out *sideways*.
