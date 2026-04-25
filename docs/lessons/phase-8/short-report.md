# Phase 8 — Reference Card

**Hardware**: Apple M4 Max · **Dataset**: GLO-30 (3601×3601, ~30m), BEV DGM 5m, BEV LiDAR 1m (8001×8001)
**Date**: 2026-04-20 to 2026-04-25

---

## 1. CRS Cheat Sheet

| EPSG | Name | Datum / Ellipsoid | Coord units | Used by |
|------|------|-------------------|-------------|---------|
| 4326 | WGS84 geographic | GRS 1980 | degrees | Copernicus GLO-30, SRTM |
| 31287 | Austria Lambert (LCC) | Bessel 1841 | metres | BEV DGM 5m |
| 3035 | LAEA Europe | GRS 1980 | metres | BEV LiDAR 1m |

**CRS discriminator**: read `ModelPixelScaleTag[0]` from GeoTIFF:
- `< 0.1` → geographic (degrees/px) → EPSG:4326
- `>= 1.0 and < 5.0` → EPSG:3035 (1 m/px)
- `>= 5.0` → EPSG:31287 (5 m/px)

**Hintertux centre (47.076211°N, 11.687592°E):**
- EPSG:31287 → (273605, 356962)
- EPSG:3035 → (4449262, 2663978)

---

## 2. GeoTIFF Affine Transform

```
col, row  →  (tiepoint_x + col * scale_x,  tiepoint_y − row * scale_y)
```
Row 0 is the northernmost row; northing increases upward, rows increase downward → sign flip on Y.

---

## 3. EPSG:31287 LCC Forward Projection (key parameters)

- Bessel 1841: a = 6,377,397 m
- φ₁ = 46°N, φ₂ = 49°N (standard parallels)
- φ₀ = 47.5°N, λ₀ = 13.333°E, FE = FN = 400,000 m

---

## 4. EPSG:3035 LAEA Forward Projection (spherical approx, ~100m accuracy)

- GRS 1980: R ≈ 6,371,000 m
- φ₀ = 52°N, λ₀ = 10°E, FE = 4,321,000 m, FN = 3,210,000 m

---

## 5. wgpu / tiff Limits

| Limit | Default | Fix | Impact |
|-------|---------|-----|--------|
| `max_storage_buffer_binding_size` | 128 MB | `required_limits: adapter.limits()` | 8001×8001 f32 = 256 MB fails |
| Texture dimension | 8192 px | same | 10001×10001 fails; 8001×8001 passes |
| `tiff` crate image size | ~128 MB | `Decoder::with_limits(Limits::unlimited())` | Large GeoTIFFs rejected |

---

## 6. CameraUniforms Extension Pattern

Reserved `_pad` fields absorb new u32 slots with **no bind group or buffer size change**.
WGSL and Rust structs must match byte-for-byte (`#[repr(C)]` + std140 alignment rules).
Keep `_pad5` to maintain 16-byte group alignment after `ao_mode`.

New fields added this phase: `shadows_enabled`, `fog_enabled`, `vat_mode`, `lod_mode`.

---

## 7. WGSL `select()` — Branchless Enum Dispatch

```wgsl
// 4-way enum: chained select, no branch, no divergence
let step_div = select(select(select(1e9, 4000.0, mode==3u), 8000.0, mode==2u), 20000.0, mode==1u);
```
Compiles to predicated moves. All threads execute the same path — correct for uniform values.

---

## 8. Viewer Key Bindings (Phase 8 additions)

| Key | Action | Default |
|-----|--------|---------|
| `.` | Toggle shadows | On |
| `,` | Toggle fog | On |
| `;` | Cycle quality (Ultra/High/Mid/Low) | High |
| `'` | Cycle LOD distance (Ultra/High/Mid/Low) | Mid |

**Quality (VAT) modes** — `step_m = dx / N`, `sphere_factor`:

| Mode | N | sphere_factor |
|------|----|--------------|
| Ultra | 20 | 0.1 |
| High  | 10 | 0.2 |
| Mid   | 5  | 0.4 |
| Low   | 3  | 0.8 |

**LOD modes** — step growth / mip divisors:

| Mode  | Step div | Mip div |
|-------|----------|---------|
| Ultra | ∞        | ∞       |
| High  | 20000    | 30000   |
| Mid   | 8000     | 15000   |
| Low   | 4000     | 8000    |

---

## 9. glyphon HUD Panel Sizing

Two independent values must both be updated when adding lines:
1. `buffer.set_size(w, h)` — text layout box; clip if `h < num_lines * line_height`
2. Background quad vertices in `build_vertices()` — separate from text layout

---

## 10. Tile Geometry at 47°N (GLO-30 / SRTM)

| Axis | Size | Fog 60km overshoots? |
|------|------|----------------------|
| E-W  | 76 km | **Always** (38 km to edge < 60 km fog) |
| N-S  | 111 km | Only when < 55 km from N/S edge |

→ **3×3 grid required**. 2×2 leaves gaps at E/W when camera is anywhere in the tile.

**Assembled 3×3 texture size**:
- Naïve: 3 × 3601 − 2 = 10,801 px — exceeds 8192 GPU limit
- Fix: outer 8 tiles at half-res (1801 px) → 1801 + 3601 + 1801 − 2 = **7,201 px** ✓
- Justified: outer tiles used only beyond ~38 km; 30m detail is subpixel there

---

## 11. 3×3 Sliding Window Policy

- **Invariant**: camera always in the centre tile
- **Trigger**: `floor(camera_lon)` or `floor(camera_lat)` changes (tile boundary crossing)
- **Action**: drop 3 trailing-edge tiles, load 3 leading-edge tiles, keep 6
- **No threshold tuning needed** — the integer floor change is exact

---

## 12. Multi-Resolution Tier Architecture (Phase 9)

| Tier | Radius | Update trigger | Preprocessing |
|------|--------|---------------|---------------|
| 30m | unlimited (3×3 grid) | tile boundary crossing | Centre: full. Outer 8: normals only |
| 5m  | 10 km (configurable) | drift > 40% of radius | Full (normals + shadows + AO) |
| 1m  | 5 km (configurable)  | drift > 40% of radius | Full |

**One background thread per tier.** Coarser tier renders as fallback while loader runs.
Communication: bounded channel size 1 (stale positions dropped). GPU upload on main thread.

**Windowed GeoTIFF extraction**: `tiff` scanline API, seek to first row, decode only needed
rows, slice columns per row. Multi-source stitching: open adjacent BEV sheets by filename
convention when window crosses source tile boundary.

---

## 13. Multi-Tier Shader Sampling

```
dist < r1m * 0.9              → pure 1m
r1m * [0.9, 1.0]              → lerp(1m, 5m)
dist < r5m * 0.9              → pure 5m
r5m * [0.9, 1.0]              → lerp(5m, 30m)
dist > r5m                    → pure 30m
radius_1m_m == 0.0            → tier not loaded; skip
```

Blend zone width = 10% of tier radius (500 m at 5 km, 1 km at 10 km).

---

## 14. Why wgpu Has No Sparse Textures

Hardware sparse textures (DX12 Tiled Resources, `VK_EXT_sparse_binding`, Metal 3 sparse) move
the residency-table lookup into fixed-function GPU silicon. wgpu deliberately excludes these
APIs. The software equivalent — texture array + shader indirection table — is conceptually
identical; the only cost is shader ALU for the lookup vs hardware TLB walk.
