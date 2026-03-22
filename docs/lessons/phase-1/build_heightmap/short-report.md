# Short Report: Phase 1 — Build Heightmap

---

## 1. File format reference

**BIL format** — flat binary array of elevation values with a sidecar `.hdr` text file.

| File | Contents |
|---|---|
| `.bil` | Raw binary: row-major, north→south, west→east within each row |
| `.hdr` | Key-value text: NROWS, NCOLS, NBITS, BYTEORDER, NODATA, ULXMAP, ULYMAP, XDIM, YDIM |
| `.blw` | World file: pixel size (°), rotation (0), origin (lon, lat of upper-left pixel) |
| `.prj` | Projection definition (WGS84 for SRTM) |

**Key `.hdr` fields for parsing:**

| Field | Our tile | Meaning |
|---|---|---|
| `BYTEORDER I` | little-endian | use `i16::from_le_bytes()` |
| `NROWS / NCOLS` | 3601 × 3601 | SRTM-1 tile size |
| `PIXELTYPE SIGNEDINT` + `NBITS 16` | `i16` per sample | |
| `NODATA -32767` | sentinel | radar dropout / water |
| `ULXMAP / ULYMAP` | 11.0 / 48.0 | origin lon/lat (upper-left = north edge) |
| `XDIM / YDIM` | 0.000277...° | 1 arc-second per sample |

**Size check:** 3601 × 3601 × 2 = 25,934,402 bytes — must match file size exactly.

---

## 2. Endianness

A multi-byte integer's bytes can be stored high-end-first (big-endian) or low-end-first (little-endian). Modern ARM and x86 chips are little-endian. `BYTEORDER I` = Intel = little-endian.

```rust
let arr: [u8; 2] = chunk.try_into().unwrap();
i16::from_le_bytes(arr)   // BYTEORDER I
i16::from_be_bytes(arr)   // BYTEORDER M
```

---

## 3. Geo reference

**Arc-second:** 1° = 60′ = 3600″. SRTM-1 samples at 1″ resolution.

| Quantity | Value | Formula |
|---|---|---|
| Metres per degree latitude | ~111,320 m | constant |
| Metres per degree longitude at lat φ | 111,320 × cos(φ) | shrinks toward poles |
| dy_meters (Hintertux, 47°N) | ~30.9 m | y_dim × 111,320 |
| dx_meters (Hintertux, 47°N) | ~21.1 m | x_dim × 111,320 × cos(47°) |

**Why cos(φ):** at latitude φ, the circle of latitude has radius R×cos(φ) (right triangle: R is hypotenuse, φ is angle from equatorial plane). Circumference = 2π×R×cos(φ). Distance per degree = 111,320×cos(φ).

**Tile naming:** by SW corner. N47E011 → 47–48°N, 11–12°E. Row 0 = north edge (48°N). Rows increase southward.

**Fencepost:** 3600 arc-second intervals → 3601 sample points. Last row/col of a tile duplicates the first of the adjacent tile — drop when stitching.

**NODATA = -32767:** radar shadow (steep north-facing slopes, deep gorges) and water surfaces. Must be interpolated before compute. For tile N47E011: 654,864 cells (~5%), all filled.

---

## 4. Heightmap struct

```rust
pub struct Heightmap {
    data:       Vec<i16>,   // elevation samples, row-major, north→south
    rows:       usize,
    cols:       usize,
    nodata:     i16,        // -32767 (already filled at parse time, kept for reference)
    origin_lat: f64,        // latitude of row 0 (north edge)
    origin_lon: f64,        // longitude of col 0 (west edge)
    dx_deg:     f64,        // degrees per column (+, east)
    dy_deg:     f64,        // degrees per row (-, south)
    dx_meters:  f64,        // real-world cell width  (for Phase 2 normals)
    dy_meters:  f64,        // real-world cell height (for Phase 2 normals)
}
```

**Coordinate access:** `lat(row) = origin_lat + row * dy_deg`, `lon(col) = origin_lon + col * dx_deg`. No per-pixel coordinate storage (would cost 200 MB for 13M cells).

**Derived fields computed at parse time:**
```rust
dx_deg    =  hdr.x_dim
dy_deg    = -hdr.y_dim                                    // negative: rows go south
dx_meters =  hdr.x_dim * 111_320.0 * hdr.origin_lat.to_radians().cos()
dy_meters =  hdr.y_dim * 111_320.0
```

---

## 5. Parsing pipeline

```
parse_bil(path: &Path)
  └── parse_hdr(&path.with_extension("hdr"))    → HdrMeta
  └── std::fs::read(path)                        → Vec<u8>
  └── size validation                            → error if mismatch
  └── chunks_exact(2).map(from_le/be_bytes)      → Vec<i16>
  └── fill_nodata(&mut data, ...)                → in-place
  └── compute dx/dy degrees and meters
  └── Ok(Heightmap { ... })
```

---

## 6. fill_nodata algorithm

**Search-outward approach** (single pass):

For each nodata cell, walk in each of 4 directions until finding a valid (non-nodata) value or hitting the boundary. Average up to 4 results.

```
for each cell (r, c):
    if data[r*cols+c] == nodata:
        collect nearest valid in: up, down, left, right
        if any found: data[r*cols+c] = average(found)
```

**Complexity:**

| | Current implementation | 4-pass improvement |
|---|---|---|
| Time | O(N² × gap), worst O(N³) | O(N²) |
| Extra memory | O(1) | O(N²) ~104 MB |
| Cache | strided column walks = poor | sequential sweeps = good |

4-pass improvement documented in `docs/improvements/algo_fill_nodata_improvement.md`.

**Known issue:** division by zero if all 4 directions hit boundary without finding valid data. Fix: return `Option<i16>`, skip cell if `None`.

---

## 7. Rust patterns used

**Result and error handling:**
```rust
type DemError = Box<dyn std::error::Error>;
// ? propagates errors, .into() converts &str/String → Box<dyn Error>
let content = std::fs::read_to_string(path)?;
return Err("message".into());
```

**HashMap lookup with error:**
```rust
let rows: usize = values.get("NROWS").ok_or("NROWS missing")?.parse()?;
```

**Byte slice → fixed array → i16:**
```rust
let arr: [u8; 2] = chunk.try_into().unwrap(); // safe: chunks_exact(2) guarantees length
i16::from_le_bytes(arr)
```

**Count sentinel values:**
```rust
data.iter().filter(|&&v| v == nodata).count()
// &&v: iter() yields &i16, filter passes &&i16, pattern destructures both layers
```

**Deref coercions:** `&PathBuf` → `&Path`, `&String` → `&str`, `&mut T` → `&T` — happen automatically when the target type is known.

**Implicit return:** last expression without semicolon is the return value. `return` only needed for early exits.

---

## 8. Memory notes

- `bil_bytes: Vec<u8>` (25.9 MB) and `bil_data: Vec<i16>` (25.9 MB) coexist → 51.8 MB peak. Drop `bil_bytes` early to halve.
- `get_value_from_neighbours` allocates `Vec<i16>` per call → ~654K heap allocations. Replace with 4 `Option<i16>` locals.

---

## 9. Verification numbers — tile N47E011

| Metric | Value |
|---|---|
| Grid size | 3601 × 3601 |
| File size | 25,934,402 bytes |
| Elevation min | 472 m (Inn valley, Innsbruck) |
| Elevation max | 3477 m (Olperer 3476 m ±1 m) |
| NODATA before fill | 654,864 (~5%) |
| NODATA after fill | 0 |
