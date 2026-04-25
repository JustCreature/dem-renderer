# Long Report: Phase 1 — Build Heightmap

A comprehensive textbook covering every concept from the build-heightmap session. A student who missed this session should be able to learn everything from this document alone.

---

## Part 1 — The Data: SRTM and the BIL Format

### 1.1 What is SRTM?

SRTM (Shuttle Radar Topography Mission) was a NASA mission in February 2000 that used a C-band radar (5.6 cm wavelength) mounted on the Space Shuttle Endeavour to measure the elevation of Earth's land surface. The shuttle flew at ~233 km altitude in an inclined orbit (28.5°), with a radar antenna pointing to the side. The radar measured round-trip travel time of microwave pulses to infer ground elevation.

The result is a near-global elevation dataset, distributed as 1°×1° tiles in two resolutions:
- **SRTM-1**: 1 arc-second per sample, ~30 m horizontal resolution, 3601×3601 samples per tile
- **SRTM-3**: 3 arc-seconds per sample, ~90 m resolution, 1201×1201 samples per tile

We use SRTM-1 (version 3, which has auxiliary void-filling applied). Tile downloaded: **N47E011**, covering 47–48°N, 11–12°E — the Zillertal Alps in Austria, home of the Hintertux glacier and the Olperer (3476 m).

### 1.2 Download formats

USGS EarthExplorer offers three formats for SRTM:

**BIL (Band Interleaved by Line):** A flat binary array of elevation values plus a `.hdr` sidecar text file. The `.hdr` declares dimensions, byte order, and data type. No external library required to parse. Correct choice for this project.

**DTED (Digital Terrain Elevation Data):** A military format developed by the NGA with complex framing headers per column. Same underlying data, more parsing work, no benefit.

**GeoTIFF:** A TIFF image with embedded georeferencing. Requires a TIFF parsing library. Adds external dependency with no learning benefit.

### 1.3 The BIL file set

The downloaded zip contains four files:

```
n47_e011_1arc_v3.bil   — 25,934,402 bytes — the elevation data
n47_e011_1arc_v3.hdr   — 306 bytes        — format metadata
n47_e011_1arc_v3.blw   — 81 bytes         — georeferencing (world file)
n47_e011_1arc_v3.prj   — 648 bytes        — projection definition (WGS84)
```

### 1.4 The .hdr file decoded

```
BYTEORDER      I          ← Intel = little-endian
LAYOUT         BIL        ← Band Interleaved by Line
NROWS          3601       ← number of rows
NCOLS          3601       ← number of columns
NBANDS         1          ← single elevation band
NBITS          16         ← 16 bits per sample
BANDROWBYTES   7202       ← 3601 samples × 2 bytes = 7202 bytes per row
TOTALROWBYTES  7202       ← same (1 band)
PIXELTYPE      SIGNEDINT  ← signed 16-bit integer = i16
ULXMAP         11         ← upper-left longitude (11°E) = origin_lon
ULYMAP         48         ← upper-left latitude (48°N) = origin_lat (north edge)
XDIM           0.000277777777777778  ← degrees per column = 1/3600°
YDIM           0.000277777777777778  ← degrees per row (magnitude)
NODATA         -32767     ← sentinel value for missing data
```

The size check: `NROWS × NCOLS × (NBITS/8) = 3601 × 3601 × 2 = 25,934,402 bytes` — exactly matches the `.bil` file size. Always verify this before parsing.

### 1.5 The .blw world file

The BLW (world file) is a 6-line Esri convention for georeferencing raster files:

```
0.0002777778    ← pixel width in degrees (east = positive)
0.0000000000    ← x rotation (always 0 for SRTM)
0.0000000000    ← y rotation (always 0 for SRTM)
-0.0002777778   ← pixel height in degrees (negative = rows go south)
11.0000000000   ← longitude of upper-left pixel centre
48.0000000000   ← latitude of upper-left pixel centre
```

The negative pixel height is the key georeferencing convention: row index increases going **south** (downward on the map), so each step adds a negative value to latitude. Row 0 is the **north** edge (48°N), row 3600 is the **south** edge (47°N).

---

## Part 2 — Endianness

### 2.1 The problem

A single `i16` value occupies 2 bytes. The number 1000 in binary:

```
0000 0011 1110 1000
└─ high byte (0x03) ─┘└─ low byte (0xE8) ─┘
```

Two bytes must be stored at two consecutive memory addresses. The question is: which byte goes first?

### 2.2 Big-endian vs little-endian

**Big-endian** — high byte at the lower address (like writing numbers left-to-right):
```
address:  0x00  0x01
data:     0x03  0xE8    → reads as 0x03E8 = 1000 ✓
```

**Little-endian** — low byte at the lower address:
```
address:  0x00  0x01
data:     0xE8  0x03    → reads as 0x03E8 = 1000 ✓ (if interpreted correctly)
```

If you read a little-endian file and interpret it as big-endian, `[0xE8, 0x03]` becomes `0xE803 = 59395` — completely wrong elevation data. This is a silent bug that's easy to ship.

### 2.3 In Rust

```rust
let bytes: [u8; 2] = [0xE8, 0x03];
i16::from_le_bytes(bytes)  // → 1000  (correct for little-endian)
i16::from_be_bytes(bytes)  // → -6141 (wrong for this data)
```

Always use the explicit form. The compiler optimises it to a `bswap` instruction (big-endian host reading big-endian data) or a no-op (little-endian host reading little-endian data).

**BYTEORDER I** (Intel) = little-endian → `from_le_bytes`.
**BYTEORDER M** (Motorola) = big-endian → `from_be_bytes`.

Modern ARM (Apple Silicon) and x86 are both little-endian. Original `.hgt` files are big-endian (designed on SPARC workstations). BIL from USGS is little-endian.

---

## Part 3 — Geographic Coordinate System

### 3.1 Arc-seconds

Geographic coordinates are measured in degrees. Degrees are subdivided like time:
- 1 degree = 60 arc-minutes
- 1 arc-minute = 60 arc-seconds
- 1 degree = 3600 arc-seconds

SRTM-1 samples terrain every **1 arc-second**. In real-world distance:
- 1 arc-second of **latitude** ≈ 30.9 m (constant — latitude lines are evenly spaced)
- 1 arc-second of **longitude** ≈ 30.9 × cos(latitude) m (shrinks toward the poles)

At Hintertux (47°N): cos(47°) ≈ 0.682, so 1 arc-second longitude ≈ 21.1 m.

### 3.2 Why longitude shrinks: the cosine derivation

Consider a point P on Earth's surface at latitude φ. Draw the right triangle from Earth's centre O:

```
    polar axis
         |
         |← R·sin(φ) (height above equator)
    ─────+──── P  ← surface point at latitude φ
         |↗ φ  ↑
         |     R·cos(φ)  ← distance from polar axis to P
         |     ↓           = radius of the latitude circle
         O──────────────  equator
```

- Hypotenuse = R (Earth's radius, ~6371 km)
- Angle between OP and the equatorial plane = φ (latitude, by definition)
- Adjacent leg = R × cos(φ) — this is the perpendicular distance from P to the polar axis

This adjacent leg is the **radius of the circle of latitude at φ**. The circle of latitude at φ has:
- Circumference = 2π × R × cos(φ)
- 1° of longitude = circumference / 360 = (2π × R / 360) × cos(φ) = **111,320 × cos(φ) metres**

At φ = 0° (equator): cos(0°) = 1.0 → full 111,320 m/degree
At φ = 47°: cos(47°) = 0.682 → 75,920 m/degree
At φ = 90° (pole): cos(90°) = 0.0 → 0 m/degree (all meridians meet)

The `cos(φ)` factor is not an approximation. It is the exact geometric relationship derived from the spherical coordinates.

### 3.3 Cell size formulas

```rust
dy_meters = y_dim * 111_320.0                                 // north-south
dx_meters = x_dim * 111_320.0 * origin_lat.to_radians().cos() // east-west
```

`to_radians()` is required because `cos()` in Rust (and all math libraries) works in radians, not degrees. Conversion: radians = degrees × π / 180.

For tile N47E011: dy_meters ≈ 30.9 m, dx_meters ≈ 21.1 m.

**This matters in Phase 2** (normal computation): the finite-difference gradient uses pixel distance. If you treat dx and dy as equal (both = 1.0), slopes appear steeper east-west than they really are.

### 3.4 Tile naming and layout

- Tiles are named by their **south-west corner**: N47E011 → SW corner at 47°N, 11°E → covers 47–48°N, 11–12°E.
- Row 0 = **north** edge (48°N). Row 3600 = south edge (47°N). **Rows increase southward.**
- Col 0 = west edge (11°E). Col 3600 = east edge (12°E). Cols increase eastward.

The "north-up trap": row 0 is at the top of the data but the top of the map (north). If you iterate rows top-to-bottom and render top-to-bottom on screen, the image is correctly oriented. Confusion arises when people assume row 0 = south.

### 3.5 Fencepost and tile stitching

A 1°×1° tile at 1 arc-second resolution has 3600 intervals between samples → **3601 sample points** (fencepost counting). The last row and last column of tile N47E011 are identical to the first row and first column of the adjacent tiles N47E012 (east) and N48E011 (north). When stitching, drop the duplicate: use rows 0–3599 from each tile, keeping only the final tile's last row/column.

### 3.6 NODATA

Value **-32767** marks cells where the SRTM radar had no usable return:
- **Water surfaces**: C-band radar scatters off water at low incidence angles rather than reflecting back to the antenna.
- **Radar shadow**: steep terrain facing away from the radar cast shadows. In the Alps, north-facing slopes in east-west valleys frequently shadow each other.
- **Steep cliffs**: near-vertical terrain can produce layover or shadow artefacts.

For tile N47E011: 654,864 NODATA cells (~5% of 12,967,201 total). This is genuine — SRTM v3 already had auxiliary void-filling applied for most areas, but steep Alpine terrain produces real gaps.

NODATA must be handled before passing data to Phase 2 compute — `-32767` in a finite-difference stencil produces a completely wrong normal.

---

## Part 4 — Rust Language Concepts

### 4.1 Result<T, E>

Rust has no exceptions. Functions that can fail return a `Result`:

```rust
enum Result<T, E> {
    Ok(T),   // success
    Err(E),  // failure
}
```

`Ok` and `Err` are enum variants — ordinary values constructed like any other enum. You return them explicitly:

```rust
fn divide(a: i32, b: i32) -> Result<i32, String> {
    if b == 0 { return Err("division by zero".into()); }
    Ok(a / b)
}
```

**`?` operator** — sugar for "propagate the error if Err, unwrap if Ok":

```rust
let n = s.parse::<i32>()?;
// expands to:
let n = match s.parse::<i32>() {
    Ok(v)  => v,
    Err(e) => return Err(e.into()),
};
```

**Handling both cases:**

```rust
match result {
    Ok(v)  => println!("got {}", v),
    Err(e) => println!("error: {}", e),
}
```

**`match` syntax:** `pattern => expression`. The `=>` separates the pattern from what to evaluate. Match is exhaustive — the compiler forces you to handle every variant.

### 4.2 Box<T> and heap allocation

`Box<T>` is a heap-allocated smart pointer. The stack holds a fixed 8-byte pointer; the actual `T` lives on the heap.

```
stack            heap
┌──────────┐    ┌──────────────────┐
│ ptr ─────┼───▶│   value of T     │
└──────────┘    └──────────────────┘
  8 bytes          size_of::<T>()
```

Used when `T`'s size is unknown at compile time — which is the case for trait objects.

### 4.3 Traits and dynamic dispatch

A **trait** is a set of method signatures a type promises to implement:

```rust
trait Greet {
    fn hello(&self) -> String;
}
impl Greet for English { fn hello(&self) -> String { "Hello".into() } }
impl Greet for German  { fn hello(&self) -> String { "Hallo".into() } }
```

**Static dispatch** (generics): the compiler generates a separate copy of the code for each concrete type at compile time. Zero runtime cost.

**Dynamic dispatch** (`dyn Trait`): the concrete type is resolved at runtime via a **vtable** — a table of function pointers.

```
fat pointer (16 bytes)
┌──────────┬──────────┐
│ data ptr │ vtable   │
└──────────┴──────────┘
                │
                ▼
          ┌─────────────────┐
          │ fn display(...) │
          │ fn source(...)  │
          │ fn drop(...)    │
          └─────────────────┘
```

`Box<dyn Error>` = heap-allocated pointer to any type implementing `Error`, with a vtable for runtime dispatch. Its size is fixed (16 bytes: data + vtable pointer) regardless of the concrete error type.

### 4.4 Type alias and error handling pattern

```rust
type DemError = Box<dyn std::error::Error>;

fn parse_hdr(path: &Path) -> Result<HdrMeta, DemError> {
    let content = std::fs::read_to_string(path)?;  // IO error propagated
    let n: usize = some_str.parse()?;               // parse error propagated
    return Err("message".into());                   // manual error from &str
}
```

`.into()` converts `&str` or `String` to `Box<dyn Error>` using trait implementations from the standard library. The target type is inferred from the function's return type.

### 4.5 String vs &str, PathBuf vs &Path

**`String`** — owned, heap-allocated, growable:
```
stack                    heap
┌───────────────────┐   ┌──────────────────┐
│ ptr ──────────────┼──▶│ h e l l o        │
│ len: 5            │   └──────────────────┘
│ capacity: 8       │
└───────────────────┘
```

**`&str`** — borrowed reference to string bytes (2 fields: pointer + length). Can point to heap, stack, or the compiled binary. Read-only.

The same relationship exists for paths:
- `PathBuf` — owned, heap-allocated path
- `&Path` — borrowed reference to path bytes

**Deref coercions** — Rust automatically converts between owned and borrowed forms when the target type is known:
- `&PathBuf` → `&Path`
- `&String` → `&str`
- `&Vec<T>` → `&[T]`
- `&mut T` → `&T`

This means a function taking `&Path` works with both `PathBuf` and `&Path` arguments — pass `&my_pathbuf` and the coercion is automatic.

**Rule:** function parameters should take the borrowed form (`&Path`, `&str`, `&[T]`) unless they need ownership.

### 4.6 HashMap

```rust
use std::collections::HashMap;
let mut map: HashMap<&str, &str> = HashMap::new();
map.insert("NROWS", "3601");
let v: Option<&&str> = map.get("NROWS");  // &V where V = &str → &&str
```

`.get()` returns `Option<&V>` — a reference to the stored value. Since the stored value is `&str`, you get `Option<&&str>`. The double reference arises because `get` adds one layer of borrowing on top of the stored type.

**Pattern to extract and parse:**
```rust
let rows: usize = map
    .get("NROWS")              // Option<&&str>
    .ok_or("NROWS missing")?   // &&str (or return Err)
    .parse()?;                 // usize (or return Err)
```

`.ok_or(msg)` converts `Option<T>` to `Result<T, E>`: `Some(v)` → `Ok(v)`, `None` → `Err(msg)`.

### 4.7 Option<T>

```rust
enum Option<T> {
    Some(T),
    None,
}
```

Represents an optional value. Rust has no null — `Option` is the explicit, type-safe replacement. The compiler forces you to handle both cases before using the inner value.

Common methods:
- `.ok_or(e)` → `Result<T, E>`: converts None to Err
- `.unwrap()` → panics if None (use only in tests/prototypes)
- `.unwrap_or(default)` → returns default if None
- `.map(|v| ...)` → transforms the inner value if Some

### 4.8 Implicit return and semicolons

In Rust, the last expression in a block is its return value — no `return` keyword needed. The semicolon changes an expression into a statement and discards the value:

```rust
fn double(x: i32) -> i32 {
    x * 2    // expression — returned
}

fn double(x: i32) -> i32 {
    x * 2;   // statement — returns (), compiler error: expected i32, found ()
}
```

Use `return` only for early exits. The final value is always implicit.

### 4.9 Closures

A closure is an anonymous function that can capture values from its surrounding scope:

```rust
let threshold = 100;
let is_high = |v: i32| v > threshold;  // captures `threshold`
```

Passed as arguments with trait bounds: `FnMut()` (can be called multiple times, may mutate captured state), `Fn()` (immutable captures), `FnOnce()` (consumed on first call).

**In iterator chains:**
```rust
data.iter()
    .filter(|&&v| v != nodata)  // &&v: iter yields &i16, filter adds & → &&i16
    .copied()                   // &&i16 → i16 (Copy type, bit-duplicate)
    .min()                      // Option<i16>
```

### 4.10 #[derive(Debug)]

Derives an automatic debug representation for printing with `{:?}`:

```rust
#[derive(Debug)]
struct HdrMeta { rows: usize, cols: usize, ... }

println!("{:?}", meta);
// HdrMeta { rows: 3601, cols: 3601, ... }
```

Cannot be derived for types with non-Debug fields. `{}` uses `Display` (human-friendly, written manually); `{:?}` uses `Debug` (developer-friendly, auto-derivable).

### 4.11 .copied() and the Copy trait

`Copy` is a marker trait for types that are safe and cheap to duplicate by copying their bits: `i16`, `u32`, `f32`, `bool`, references. When an iterator yields `&T` where `T: Copy`, `.copied()` converts each `&T` to `T`:

```rust
// without .copied(): iterator yields &i16
let min: Option<&i16> = data.iter().min();

// with .copied(): iterator yields i16
let min: Option<i16> = data.iter().copied().min();
```

`.cloned()` is the equivalent for `Clone` types (heap-allocated, more expensive).

---

## Part 5 — Heightmap Struct Design

### 5.1 Why store origin + step instead of per-pixel coordinates

The 3601×3601 grid has 12,967,201 cells. Storing `(lat, lon)` as `f64` pairs per cell:
- 12,967,201 × 16 bytes = **207 MB** — more than the elevation data itself

Coordinates are derived from two values — store those instead:

```rust
fn lat(&self, row: usize) -> f64 { self.origin_lat + row as f64 * self.dy_deg }
fn lon(&self, col: usize) -> f64 { self.origin_lon + col as f64 * self.dx_deg }
```

Two multiplications per query, zero extra memory.

### 5.2 Why Vec<i16> is owned, not borrowed

`Heightmap` is returned from `parse_bil` and lives beyond the function. A `&Vec<i16>` would borrow from something inside `parse_bil` — that something would be dropped when the function returns, leaving a dangling reference. Rust catches this at compile time.

Owned fields (`Vec<i16>`) transfer ownership to the struct. The struct's lifetime becomes the data's lifetime.

### 5.3 Sign conventions

`dy_deg` is **negative** because rows increase southward (latitude decreases). The formula `lat(row) = origin_lat + row * dy_deg` gives decreasing latitude as row increases — correct.

`dx_deg` is **positive** because columns increase eastward (longitude increases).

`dy_deg = -y_dim` where `y_dim` from the `.hdr` is always positive (a magnitude). `dx_deg = +x_dim`.

### 5.4 Why carry nodata in the struct

After `fill_nodata`, no cell equals `-32767`. But carrying the `nodata` field:
1. Documents the convention for anyone reading the struct
2. Allows future code to handle tiles where fill wasn't run
3. Is trivially cheap (2 bytes)

---

## Part 6 — Implementation: parse_hdr

### 6.1 Approach

Read the `.hdr` file into a `String`, iterate lines, split each on whitespace, insert into a `HashMap<&str, &str>`. Then look up each required key and parse to the correct type.

Using `.lines()` instead of `.split("\n")` handles both Unix (`\n`) and Windows (`\r\n`) line endings — important since `.hdr` files are sometimes distributed with Windows line endings.

### 6.2 The &&str explanation

```rust
let map: HashMap<&str, &str> = HashMap::new();
let v: Option<&&str> = map.get("KEY");
```

`HashMap::get` has signature `fn get(&self, k: &K) -> Option<&V>`. Here `V = &str`, so `get` returns `Option<&&str>` — a reference to the stored `&str`. To compare:

```rust
let byteorder = map.get("BYTEORDER").ok_or("missing")?; // type: &&str
let little_endian = *byteorder == "I";                   // * dereferences &&str → &str
```

### 6.3 .into() type inference

```rust
fn parse_hdr(path: &Path) -> Result<HdrMeta, DemError> {
    return Err("message".into());
}
```

The compiler knows the return type is `Result<HdrMeta, DemError>` = `Result<HdrMeta, Box<dyn Error>>`. So `Err(...)` must contain a `Box<dyn Error>`. `.into()` finds the `From<&str> for Box<dyn Error>` implementation in std and uses it. The target type is entirely inferred from context — `.into()` itself carries no type information.

---

## Part 7 — Implementation: fill_nodata

### 7.1 Why fill before compute

NODATA = -32767. Passed into a finite-difference stencil:
```
nx = h[x-1][y] - h[x+1][y]
```
If `h[x+1][y] = -32767`, `nx = h[x-1][y] - (-32767)` = a huge positive number. The resulting normal vector points in a completely wrong direction and corrupts the shadow computation. Every downstream phase assumes valid elevation data.

### 7.2 Algorithm: search outward

For each NODATA cell, walk in 4 cardinal directions independently. Stop each walk at the first valid (non-NODATA) value. Average whatever was found (1–4 values).

```
for each cell (r, c):
    if data[r*cols+c] == nodata:
        up    = first valid value walking r-1, r-2, ... or None
        down  = first valid value walking r+1, r+2, ... or None
        left  = first valid value walking c-1, c-2, ... or None
        right = first valid value walking c+1, c+2, ... or None
        if any found:
            data[r*cols+c] = average(found values)
```

Single pass — no outer loop needed. Each NODATA cell is independent.

### 7.3 Index arithmetic

Flat array indexing: `index = row * cols + col`.

Boundary checks are mandatory:
- up: only if `r > 0`
- down: only if `r < rows - 1`
- left: only if `c > 0`
- right: only if `c < cols - 1`

### 7.4 The break requirement

Without `break`, the walk continues after finding the first valid value and collects all valid values in that direction:

```rust
// WRONG — collects multiple values
if data[cell] != nodata { neighbours.push(data[cell]); }

// CORRECT — stops at nearest
if data[cell] != nodata { neighbours.push(data[cell]); break; }
```

A distant valid cell should not influence the fill equally with a nearby one.

### 7.5 Avoiding integer overflow

`i16` max = 32,767. Sum of 4 `i16` values can reach ~131,000 — overflows `i16`. Accumulate in `i32`:

```rust
let sum: i32 = neighbours.iter().map(|&v| v as i32).sum();
let count: i32 = neighbours.len() as i32;
(sum / count) as i16
```

### 7.6 Complexity analysis

- Outer loop: O(rows × cols) = O(N²) — visits every cell once
- Per NODATA cell: O(max(rows, cols)) = O(N) worst case — searches entire row/column
- Total worst case: **O(N³)**
- Expected case for SRTM: O(N² + nodata_count × avg_gap) — much better in practice

**Known issue:** if `count == 0` (all 4 directions reach the boundary without finding valid data — theoretically possible if an entire row and column are NODATA), division by zero panics. Fix: return `Option<i16>`, skip the cell if `None`.

### 7.7 Memory inefficiency in current implementation

`get_value_from_neighbours` allocates a `Vec<i16>` on every call. For 654,864 NODATA cells, this is 654,864 heap allocations of 0–4 elements each. The allocator overhead dominates the actual computation.

Fix: replace with 4 local `Option<i16>` variables — zero allocation, same logic:
```rust
let up:    Option<i16> = None;
let down:  Option<i16> = None;
let left:  Option<i16> = None;
let right: Option<i16> = None;
// ... walks assign Some(v) ...
let found: Vec<i32> = [up, down, left, right].iter()
    .filter_map(|&v| v)
    .map(|v| v as i32)
    .collect();
```

### 7.8 The 4-pass O(N²) improvement

Instead of each NODATA cell searching outward independently, precompute the nearest valid value in each direction for all cells in a single linear scan:

**Left→right sweep (per row):**
```
last_seen = None
for c in 0..cols:
    if data[r,c] != nodata: last_seen = Some(data[r,c])
    else: left_fill[r,c] = last_seen
```

Four sweeps (left→right, right→left, top→bottom, bottom→top), each O(N²). Total: O(N²). See `docs/improvements/algo_fill_nodata_improvement.md`.

---

## Part 8 — Implementation: parse_bil

### 8.1 Reading bytes

```rust
let bytes: Vec<u8> = std::fs::read(bil_path)?;
```

`std::fs::read` reads the entire file into a heap-allocated `Vec<u8>`.

### 8.2 Size validation

```rust
let expected = hdr.rows * hdr.cols * 2;
if bytes.len() != expected {
    return Err(format!("expected {} bytes, got {}", expected, bytes.len()).into());
}
```

`format!` returns a `String`. `.into()` converts `String` → `Box<dyn Error>`.

### 8.3 Byte conversion

```rust
let data: Vec<i16> = bytes.chunks_exact(2).map(|chunk| {
    let arr: [u8; 2] = chunk.try_into().unwrap();
    if hdr.little_endian { i16::from_le_bytes(arr) }
    else                 { i16::from_be_bytes(arr) }
}).collect();
```

`chunks_exact(2)` yields `&[u8]` slices of exactly 2 bytes. `try_into()` converts `&[u8]` → `[u8; 2]` (fixed-size array required by `from_le_bytes`). The `unwrap()` is safe because `chunks_exact` guarantees length — the compiler just can't prove it statically.

### 8.4 Peak memory

`bytes: Vec<u8>` (25.9 MB) and `data: Vec<i16>` (25.9 MB) coexist → 51.8 MB peak. `bytes` is no longer needed after the `.collect()`. To halve peak memory, drop it explicitly:

```rust
drop(bytes);
```

Or scope the conversion in a block so `bytes` is dropped at the closing brace.

### 8.5 Derived fields

```rust
let dx_deg    =  hdr.x_dim;
let dy_deg    = -hdr.y_dim;                // negative: rows go south
let dy_meters =  hdr.y_dim * 111_320.0;
let dx_meters =  hdr.x_dim * 111_320.0 * hdr.origin_lat.to_radians().cos();
```

`origin_lat` is used for `cos(lat)` — the north edge of the tile (48°N for N47E011). Accurate enough for a 1°×1° tile.

---

## Part 9 — Verification

After parsing tile N47E011:

| Check | Result | Expected |
|---|---|---|
| Grid size | 3601 × 3601 | SRTM-1 ✓ |
| Elevation min | 472 m | Inn valley near Innsbruck (~400–500 m) ✓ |
| Elevation max | 3477 m | Olperer 3476 m — within 1 m ✓ |
| NODATA before fill | 654,864 (~5%) | Genuine alpine radar shadow ✓ |
| NODATA after fill | 0 | All filled ✓ |
| Range unchanged by fill | 472–3477 m | No extreme values introduced ✓ |

The 3477 m maximum matching the Olperer to within 1 m is strong validation. The Olperer is the highest peak in the Tux Alps (3476 m) directly above Hintertux. The 1 m discrepancy is expected from discrete grid sampling — the peak summit may fall between sample points.

---

## Part 10 — profiling::timed Generalisation

The original `timed` only timed `FnMut()` (closures returning nothing). To time functions that produce values, make it generic over the return type:

```rust
pub fn timed<R, F: FnMut() -> R>(label: &str, mut f: F) -> (u64, R) {
    let t0 = now();
    let result = f();
    let t1 = now();
    println!("{},{}", label, t1 - t0);
    (t1 - t0, result)
}
```

Call site:
```rust
let (ticks, heightmap) = profiling::timed("build heightmap", || {
    dem_io::parse_bil(tile_path).unwrap()
});
```

For closures returning `()`, `R` infers as `()` and the return is `(u64, ())`. Existing callers updated to destructure: `let (ticks, _) = timed(...)`.
