# Session Log: Phase 1 — Build Heightmap
**Date:** 2026-03-22
**Branch:** a_1

---

## What was covered

### Data acquisition
- Discussed download format options on USGS EarthExplorer: BIL, DTED, GeoTIFF. Chose BIL — flat binary with a sidecar .hdr, parseable without external libraries.
- Downloaded SRTM-1 tile N47E011 (Zillertal Alps / Hintertux). Covers 47–48°N, 11–12°E.
- File set: `n47_e011_1arc_v3.bil` (25,934,402 bytes), `.hdr`, `.blw`, `.prj`.

### File format decoding
- `.hdr`: BYTEORDER I (little-endian), NROWS/NCOLS 3601, NBITS 16, PIXELTYPE SIGNEDINT, NODATA -32767, ULXMAP 11, ULYMAP 48, XDIM/YDIM 0.000277...°
- `.blw` world file: pixel size 0.000277°, negative Y = rows go north→south, origin at 11°E/48°N.
- Size check: 3601 × 3601 × 2 = 25,934,402 bytes — matches exactly.
- Endianness explained: byte order of multi-byte integers in memory. `from_le_bytes` / `from_be_bytes`.

### Geo 101
- Arc-second: 1° = 60' = 3600". 1 arc-second ≈ 30.9 m N-S.
- Tile naming: SW corner. N47E011 → 47–48°N, 11–12°E. Row 0 = north edge (48°N).
- Fencepost: 3600 intervals → 3601 samples per edge, overlapping border with adjacent tiles.
- NODATA = -32767: radar dropout over water and steep north-facing slopes. At 5%, genuine for alpine tile.
- Cell size asymmetry: dx ≈ 21 m, dy ≈ 31 m at 47°N. Pixels are not square on the ground.
- cos(lat) derivation: latitude circle radius = R×cos(φ) from right triangle geometry. dx_meters = x_dim × 111,320 × cos(lat).

### Rust concepts introduced
- `Result<T,E>`, `Ok(T)`, `Err(E)`, `?` operator, explicit `return` vs implicit last-expression.
- `Box<dyn Error>`: heap allocation, dynamic dispatch via vtable (fat pointer = 16 bytes).
- Traits: contract of methods a type implements. Standard traits: Debug, Display, Copy, Error, Deref.
- `#[derive(Debug)]`, `{:?}` vs `{}`.
- `String` vs `&str`, `PathBuf` vs `&Path` — owned vs borrowed, deref coercions.
- `HashMap<K,V>`: `.get()` returns `Option<&V>`, `.insert(k,v)`. `&&str` explained.
- `Option<T>`: `Some(T)` / `None`, `.ok_or()`, `.unwrap_or()`.
- `.into()` type inference driven by return type annotation.
- Closures: `|arg| expr`. `FnMut`.
- `.copied()` for `Copy` types in iterator chains.
- Struct construction: named fields required, shorthand when variable name matches field name.
- `match` and `=>` syntax, exhaustive pattern matching.

### Heightmap struct design decisions
- Store origin + step, not per-pixel coordinates (13M × 16 bytes = 200 MB wasted).
- `data: Vec<i16>` owned — struct must outlive the function, borrows would dangle.
- `dy_deg` is negative (rows go south), `dx_deg` positive (cols go east).
- `nodata` field carried so downstream phases never need to re-discover the sentinel.
- `dx_meters` / `dy_meters` precomputed at parse time — Phase 2 normals need them.

### Implementation built
- `parse_hdr`: reads `.hdr` → `HashMap<&str, &str>` → `HdrMeta`. Used `.lines()` for CRLF safety.
- `fill_nodata`: search-outward approach — each nodata cell walks 4 directions, stops at first valid value per direction, averages up to 4 results.
- `parse_bil`: validates byte count, converts bytes via `chunks_exact(2)` + `try_into().unwrap()` + `from_le/be_bytes`, calls `fill_nodata`, computes derived fields, returns `Heightmap`.

### Verification numbers
- Elevation min: 472 m (Inn valley near Innsbruck)
- Elevation max: 3477 m (Olperer 3476 m — match within 1 m of known peak)
- NODATA before fill: 654,864 (~5% of tile)
- NODATA after fill: 0

### Issues found and fixed
- `parse_hdr`: hardcoded path instead of using parameter — fixed.
- `parse_hdr`: `split("\n")` → `.lines()` for CRLF safety.
- `fill_nodata`: missing `break` — was collecting all valid values per direction, not just nearest — fixed.
- `fill_nodata`: `Vec<i16>` heap allocation per nodata cell (~654K allocations) — documented.
- `fill_nodata`: division by zero if all 4 directions hit boundary without finding valid data — `Option<i16>` return type fix pending.
- `parse_bil`: `bil_bytes` + `bil_data` live simultaneously → 51.8 MB peak (double needed) — documented.

### Other changes
- `profiling::timed` made generic over return type: `FnMut() -> R`, returns `(u64, R)`.
- `docs/improvements/algo_fill_nodata_improvement.md` written: 4-pass O(N²) sweep algorithm.
- `docs/planning/global_plan.md` updated: Geo 101 section added after Phase 1 step 1.
