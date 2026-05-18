# Multi-tier terrain misalignment: root cause and fix

## Symptom

When flying to a new area the 5 m BEV tier visibly jumps relative to the
Copernicus 30 m base.  A manual alignment correction tuned for one location
(e.g. demo_tirol: dx=-270 m, dy=-360 m, rot=2.03°) did not transfer to other
locations — the offset was different everywhere.

## Root cause: missing datum shift in proj4 string

The pipeline is:

```
5 m tile CRS origin (EPSG:31287 metres)
    → to_wgs84() via proj4rs
    → from_wgs84() back into base tile metre space
    → cross_crs_world_origin() returns (origin_x, origin_y) for the GPU upload
```

EPSG:31287 (MGI / Austria Lambert) uses the **Bessel 1841 ellipsoid** whose
geocentric origin is offset from WGS84 by roughly dx=577 m, dy=90 m, dz=464 m
(the 7-parameter MGI → WGS84 Helmert transform, EPSG transformation 1618).

Neither of our two proj4 string sources encoded this shift:

| Source | What it produced | Why |
|---|---|---|
| `crs-definitions` EPSG fallback | `+proj=lcc … +ellps=bessel` — no `+towgs84` at all | crs-definitions follows WKT2/ISO 19111 policy: datum shifts are separate from CRS definitions |
| `proj4wkt` from tile WKT | `… +towgs84=0,0,0,0,0,0,0` | The BEV tile's embedded WKT has no `TOWGS84[…]` node (same WKT2 policy); proj4wkt emits the zero-shift fallback |

Result: `proj4rs` treated the Bessel 1841 geocentric origin as identical to
WGS84 — wrong by ~450 m total.  The error is **not spatially uniform**: the
projected component of the geocentric shift vector varies with geographic
position, so the offset at one location differs from the offset at another by
~20–50 m across Austria.  That is why a single manual correction could not
work everywhere.

The Copernicus base (EPSG:4326, WGS84) and the 1 m BEV tier (EPSG:3035,
ETRS89/GRS80) are both effectively WGS84 and were never affected.

## Fix (`crates/dem_io/src/crs.rs`)

After resolving the proj4 string (from either the WKT or the EPSG fallback),
`proj4_from_keys` now applies a lookup table of 7-parameter Helmert shifts
keyed by EPSG code:

```
if EPSG in table AND (proj4 has no +towgs84  OR  +towgs84=0,0,0,0,0,0,0):
    inject/replace with correct +towgs84 from table
```

Non-zero values already embedded in the file are left untouched — this handles
future files that do embed their own datum shift correctly.

### Lookup table (crs.rs `epsg_towgs84`)

| Datum | EPSG codes | +towgs84 |
|---|---|---|
| MGI (Austria, Bessel 1841) | 4312, 31254–31259, 31281–31290 | 577.326,90.129,463.919,5.137,1.474,5.297,2.4232 |
| DHDN (Germany, Bessel 1841) | 4314, 31466–31469 | 598.1,73.7,418.2,0.202,0.045,-2.455,6.7 |
| OSGB 1936 (Britain, Airy 1830) | 4277, 27700 | 446.448,-125.157,542.06,0.1502,0.247,0.8421,-20.4894 |
| ED50 (Europe, Intl 1924) | 4230, 23028–23038 | -87,-98,-121,0,0,0,0 *(±5 m by region)* |
| CH1903 (Switzerland, Bessel 1841) | 4149, 21781 | 674.374,15.056,405.346,0,0,0,0 |
| Tokyo (Japan, Bessel 1841) | 4301, 30161–30169 | -147,506,687,0,0,0,0 *(±30 m by island)* |
| NZGD49 (New Zealand, Intl 1924) | 4272, 27200 | 59.47,-5.04,187.44,0.47,-0.1,1.024,-4.5993 |

Source: EPSG Geodetic Parameter Dataset (epsg.org).

## Expected result

The ~450 m systematic offset between the 5 m BEV tier and the Copernicus base
drops to ~1–3 m everywhere in Austria — within the noise of the pixel grid
(5 m/px).  The manual alignment correction in config is no longer necessary.
The position-dependent variation also disappears because the Helmert transform
is applied consistently at every tile load.

## Why not use an NTv2 grid file?

NTv2 (.gsb) grid files give sub-meter accuracy for national datums but require
PROJ 6+ (the `proj` C library crate).  `proj4rs` (our dependency) is a
pure-Rust PROJ4 port that does not support NTv2 grids.  For 5 m-resolution
data a 1–3 m Helmert residual is well below one pixel and does not need a
grid correction.
