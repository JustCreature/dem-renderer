//! Shared fixture metadata for the integration tests.
//!
//! Fixtures live in `tests/fixtures/` and are 512×512 windows cut with gdal from
//! real-world DEMs, one per CRS family. See the crate's test plan for how they
//! were produced. Each `Fx` row records what the reader is expected to derive so
//! the table-driven tests in `geotiff_real.rs` can assert against it.

use std::path::PathBuf;

pub struct Fx {
    /// File name under `tests/fixtures/`.
    pub file: &'static str,
    /// `crs_epsg` that `parse_geotiff_auto` should report
    /// (`projected_epsg.or(geographic_epsg).unwrap_or(0)`).
    pub epsg: u32,
    /// Whether the resolved proj4 is a geographic (lon/lat) CRS.
    pub geographic: bool,
    /// IFD-0 pixel scale: m/px for projected, deg/px for geographic.
    pub px_scale: f64,
    /// WGS84 latitude plausibility box (min, max) the tile centre must fall in.
    pub lat: (f64, f64),
    /// WGS84 longitude plausibility box (min, max).
    pub lon: (f64, f64),
}

pub fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// All fixtures with their expected, gdal-verified properties.
pub const FIXTURES: &[Fx] = &[
    // Austria BEV DGM 5 m, MGI / Austria Lambert. Resolved via WKT/EPSG with the
    // built-in MGI towgs84 Helmert shift injected. Centre 13.35°E 47.71°N.
    Fx {
        file: "austria_dgm_5m_lambert.tif",
        epsg: 31287,
        geographic: false,
        px_scale: 5.0,
        lat: (46.0, 49.0),
        lon: (12.0, 15.0),
    },
    // ETRS89-LAEA Europe 1 m (Tirol). Centre 11.37°E 47.18°N.
    Fx {
        file: "tirol_1m_etrs89laea.tif",
        epsg: 3035,
        geographic: false,
        px_scale: 1.0,
        lat: (46.0, 49.0),
        lon: (10.0, 13.0),
    },
    // New Zealand LiDAR 1 m, NZGD2000 / NZTM2000 — single IFD (no overviews).
    // Centre 171.45°E 43.39°S.
    Fx {
        file: "newzealand_1m_nztm_no_ifd.tif",
        epsg: 2193,
        geographic: false,
        px_scale: 1.0,
        lat: (-45.0, -42.0),
        lon: (170.0, 173.0),
    },
    // NOAA Oahu LiDAR 1 m, NAD83(PA11) / UTM 4N. NoData sentinel −3.4e38.
    // Centre 157.81°W 21.26°N.
    Fx {
        file: "oahu_diamondhead_1m_utm4n.tif",
        epsg: 6634,
        geographic: false,
        px_scale: 1.0,
        lat: (20.0, 23.0),
        lon: (-159.0, -156.0),
    },
    // PGC HMA 8 m mosaic, user-defined Albers encoded as inline GeoKeys (3072 =
    // 32767). No projected EPSG → crs_epsg falls back to the geographic base 4326.
    // Centre 87.0°E 27.93°N (near Everest).
    Fx {
        file: "everest_hma_8m_albers_inline.tif",
        epsg: 4326,
        geographic: false,
        px_scale: 8.0,
        lat: (26.0, 29.0),
        lon: (85.0, 89.0),
    },
    // Kartverket DTM1 1 m, ETRS89 / UTM 33N (Norway — the directory name says NZ
    // but the CRS is European). Centre 9.89°E 59.15°N.
    Fx {
        file: "dtm1_1m_utm33n.tif",
        epsg: 25833,
        geographic: false,
        px_scale: 1.0,
        lat: (57.0, 61.0),
        lon: (8.0, 12.0),
    },
    // Copernicus GLO-30, WGS84 geographic. ~30 m ⇒ 0.0002777°/px. Centre 11.5°E 47.5°N.
    Fx {
        file: "copernicus_n47e011_30m_wgs84.tif",
        epsg: 4326,
        geographic: true,
        px_scale: 0.000_277_777_777_778,
        lat: (46.0, 49.0),
        lon: (10.0, 13.0),
    },
];
