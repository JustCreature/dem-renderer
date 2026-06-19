use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek};
use std::path::Path;
use tiff::decoder::Decoder;
use tiff::tags::Tag;

use crate::DemError;

/// Returns true if the proj4 string describes a geographic (lon/lat) CRS.
pub fn is_geographic(proj4: &str) -> bool {
    proj4.contains("longlat") || proj4.contains("latlong")
}

/// Resolve the proj4 string for the CRS of a GeoTIFF file.
///
/// Tries three discovery paths in order:
/// 1. WKT from GeoAsciiParamsTag (34737) via GeoKey 3073 / 2049 — proj4wkt.
/// 2. Inline GeoKey-encoded projection: synthesise from ProjCoordTransGeoKey (3075)
///    and parameter keys (3078–3095). Used when 3072 is the user-defined sentinel
///    (32767) and no usable WKT is present — e.g. PGC's HMA 8 m mosaics.
/// 3. EPSG code from GeoKey 3072 (projected) or 2048 (geographic) — crs-definitions.
pub fn tile_proj4(path: &Path) -> Result<String, DemError> {
    proj4_from_keys(&read_geo_key_data(path)?)
}

/// Reads GeoKeyDirectoryTag (34735) and returns the projected EPSG (3072) if real,
/// otherwise the geographic EPSG (2048). The user-defined sentinel 32767 is treated
/// as "absent" and never returned.
pub fn get_tile_epsg(path: &Path) -> Result<u32, DemError> {
    let data = read_geo_key_data(path)?;
    data.projected_epsg
        .or(data.geographic_epsg)
        .ok_or_else(|| "no CRS GeoKey (3072/2048) found in GeoKeyDirectory".into())
}

/// Resolve proj4 from already-read GeoKeyData — avoids a second file open when the
/// caller also needs the EPSG code from the same read.
pub(crate) fn proj4_from_keys(data: &GeoKeyData) -> Result<String, DemError> {
    let mut p4 = if let Some(ref wkt) = data.wkt_candidate {
        proj4wkt::wkt_to_projstring(wkt).map_err(|e| {
            DemError::from(format!("WKT found but proj4wkt failed to parse it: {e}"))
        })?
    } else if data.inline.contains_key(&3075) {
        proj4_from_inline_geokeys(data)?
    } else if let Some(epsg) = data.projected_epsg.or(data.geographic_epsg) {
        epsg_to_proj4(epsg)?
    } else {
        return Err(DemError::from(
            "no CRS metadata: file has no WKT, no ProjCoordTransGeoKey (3075), and no EPSG code",
        ));
    };

    // proj4wkt defaults to +towgs84=0,0,0,0,0,0,0 when the WKT has no TOWGS84 node
    // (WKT2/ISO 19111 intentionally omits datum shift from CRS definitions).
    // crs-definitions also omits +towgs84 for most non-WGS84 datums.
    // Override with known 7-parameter Helmert values keyed on either the projected
    // or geographic EPSG — the datum shift is a property of the underlying datum,
    // so checking either slot catches it.
    for epsg in [data.projected_epsg, data.geographic_epsg]
        .into_iter()
        .flatten()
    {
        let Some(params) = epsg_towgs84(epsg) else {
            continue;
        };
        let replacement = format!("+towgs84={params}");
        if let Some(pos) = p4.find("+towgs84=") {
            let end = p4[pos..].find(' ').map_or(p4.len(), |i| pos + i);
            let existing = &p4[pos..end];
            // Only override the zero-shift fallback; respect non-zero values the file provides.
            let is_zero = existing
                .split('=')
                .nth(1)
                .unwrap_or("")
                .split(',')
                .all(|s| s.trim().parse::<f64>().unwrap_or(1.0) == 0.0);
            if is_zero {
                p4.replace_range(pos..end, &replacement);
            }
        } else {
            p4.push(' ');
            p4.push_str(&replacement);
        }
        break;
    }

    Ok(p4)
}

/// Synthesise a proj4 string from inline GeoKey-encoded projection metadata.
///
/// This handles the original GeoTIFF 1.0 CRS encoding still emitted by tools like
/// the Polar Geospatial Center's HMA pipeline: ProjectedCSTypeGeoKey (3072) is the
/// user-defined sentinel (32767), and the projection is described by an inline
/// method code (3075) plus parameter keys (3078–3095) that reference floating-point
/// values in GeoDoubleParamsTag (34736).
fn proj4_from_inline_geokeys(data: &GeoKeyData) -> Result<String, DemError> {
    let trans_code = *data
        .inline
        .get(&3075)
        .expect("caller must verify ProjCoordTransGeoKey (3075) is present");
    let proj_name = transform_to_proj4_name(trans_code as u16).ok_or_else(|| {
        DemError::from(format!(
            "ProjCoordTransGeoKey={trans_code} is not a supported projection method"
        ))
    })?;

    // Collect projection parameters into a sorted map. proj4 args are order-independent;
    // sorting just gives deterministic, easily-diffable output for debugging.
    let mut params: BTreeMap<&'static str, f64> = BTreeMap::new();
    for (&key_id, &idx) in &data.double_refs {
        let Some(flag) = projection_param_to_proj4_flag(key_id) else {
            continue;
        };
        let Some(&val) = data.doubles.get(idx as usize) else {
            continue;
        };
        params.insert(flag, val);
    }

    let mut parts = vec![format!("+proj={proj_name}")];
    for (flag, val) in &params {
        parts.push(format!("{flag}={val}"));
    }

    let geog = data.geographic_epsg.unwrap_or(4326);
    parts.push(geographic_to_datum_or_ellps(geog).to_string());

    let units = data.inline.get(&3076).copied().unwrap_or(9001);
    parts.push(linear_units_to_proj4_flag(units).to_string());

    parts.push("+no_defs".to_string());
    Ok(parts.join(" "))
}

/// GeoTIFF ProjCoordTransGeoKey (3075) → proj4 +proj= name.
/// Covers the projection methods listed in the GeoTIFF spec annex 6.3.3.3.
fn transform_to_proj4_name(code: u16) -> Option<&'static str> {
    Some(match code {
        1 => "tmerc",   // CT_TransverseMercator
        7 => "merc",    // CT_Mercator
        8 => "lcc",     // CT_LambertConfConic_2SP
        9 => "lcc",     // CT_LambertConfConic_Helmert (1SP)
        10 => "laea",   // CT_LambertAzimEqualArea
        11 => "aea",    // CT_AlbersEqualArea
        12 => "aeqd",   // CT_AzimuthalEquidistant
        13 => "eqdc",   // CT_EquidistantConic
        14 => "stere",  // CT_Stereographic
        15 => "stere",  // CT_PolarStereographic
        16 => "sterea", // CT_ObliqueStereographic
        17 => "eqc",    // CT_Equirectangular
        18 => "cass",   // CT_CassiniSoldner
        19 => "gnom",   // CT_Gnomonic
        20 => "mill",   // CT_MillerCylindrical
        21 => "ortho",  // CT_Orthographic
        22 => "poly",   // CT_Polyconic
        23 => "robin",  // CT_Robinson
        24 => "sinu",   // CT_Sinusoidal
        25 => "vandg",  // CT_VanDerGrinten
        _ => return None,
    })
}

/// Projection parameter GeoKey ID (3078–3095) → proj4 flag.
/// Several keys map to the same flag (NatOrigin vs FalseOrigin vs Center variants);
/// in practice a given projection uses one variant or the other, not both.
fn projection_param_to_proj4_flag(key_id: u16) -> Option<&'static str> {
    Some(match key_id {
        3078 => "+lat_1",               // ProjStdParallel1GeoKey
        3079 => "+lat_2",               // ProjStdParallel2GeoKey
        3080 | 3084 => "+lon_0",        // ProjNatOriginLong / ProjFalseOriginLong
        3081 | 3085 | 3089 => "+lat_0", // ProjNatOriginLat / ProjFalseOriginLat / ProjCenterLat
        3082 | 3086 | 3090 => "+x_0",   // FalseEasting / FalseOriginEasting / CenterEasting
        3083 | 3087 | 3091 => "+y_0",   // FalseNorthing / FalseOriginNorthing / CenterNorthing
        3088 => "+lonc",                // ProjCenterLongGeoKey (oblique mercator)
        3092 | 3093 => "+k_0",          // ProjScaleAtNatOrigin / ProjScaleAtCenter
        3094 => "+alpha",               // ProjAzimuthAngleGeoKey
        3095 => "+lon_0",               // ProjStraightVertPoleLongGeoKey (polar stere)
        _ => return None,
    })
}

/// GeographicTypeGeoKey (2048) → proj4 +datum= or +ellps= argument.
/// Falls back to WGS84 for unknown codes (including a stray 32767 user-defined sentinel).
fn geographic_to_datum_or_ellps(geog_epsg: u32) -> &'static str {
    match geog_epsg {
        4326 => "+datum=WGS84",
        4269 => "+datum=NAD83",
        4267 => "+datum=NAD27",
        4258 => "+ellps=GRS80", // ETRS89 (effectively WGS84 to within cm)
        4314 | 4312 => "+ellps=bessel", // DHDN / MGI — Helmert shift applied separately
        4277 => "+ellps=airy",  // OSGB36
        4230 => "+ellps=intl",  // ED50 (International 1924)
        _ => "+datum=WGS84",
    }
}

/// ProjLinearUnitsGeoKey (3076) → proj4 +units= argument.
fn linear_units_to_proj4_flag(code: u32) -> &'static str {
    match code {
        9001 => "+units=m",
        9002 => "+units=ft",
        9003 => "+units=us-ft",
        _ => "+units=m",
    }
}

/// 7-parameter Helmert shift to WGS84 for well-known non-WGS84 national datums.
/// Returns the comma-separated dx,dy,dz,rx,ry,rz,s string for +towgs84=.
/// Parameters sourced from the EPSG Geodetic Parameter Dataset (epsg.org).
fn epsg_towgs84(epsg: u32) -> Option<&'static str> {
    match epsg {
        // MGI (Militärgeographisches Institut) — Austria, Bessel 1841
        // EPSG transformation 1618
        4312 | 31254..=31259 | 31281..=31290 => {
            Some("577.326,90.129,463.919,5.137,1.474,5.297,2.4232")
        }
        // DHDN (Deutsches Hauptdreiecksnetz) — Germany, Bessel 1841
        // EPSG transformation 1673
        4314 | 31466..=31469 => Some("598.1,73.7,418.2,0.202,0.045,-2.455,6.7"),
        // OSGB 1936 — Great Britain, Airy 1830
        // EPSG transformation 1314
        4277 | 27700 => Some("446.448,-125.157,542.06,0.1502,0.247,0.8421,-20.4894"),
        // ED50 (European Datum 1950) — International 1924
        // General European value; actual residual varies ±5 m by sub-region
        4230 | 23028..=23038 => Some("-87,-98,-121,0,0,0,0"),
        // Swiss CH1903 — Bessel 1841
        4149 | 21781 => Some("674.374,15.056,405.346,0,0,0,0"),
        // Tokyo — Bessel 1841; continental Japan average (varies by island ±30 m)
        4301 | 30161..=30169 => Some("-147,506,687,0,0,0,0"),
        // NZGD49 (New Zealand Geodetic Datum 1949) — International 1924
        // EPSG transformation 1564
        4272 | 27200 => Some("59.47,-5.04,187.44,0.47,-0.1,1.024,-4.5993"),
        _ => None,
    }
}

/// Transform native CRS coordinates → WGS84 (lat_deg, lon_deg).
/// Handles radians↔degrees conversion for geographic projections internally.
pub fn to_wgs84(x: f64, y: f64, proj4: &str) -> Result<(f64, f64), DemError> {
    use proj4rs::proj::Proj;
    use proj4rs::transform::transform;

    let src = Proj::from_proj_string(proj4)?;
    let wgs84 = Proj::from_proj_string("+proj=longlat +datum=WGS84 +no_defs")?;

    // Geographic source: proj4rs expects radians
    let (mut px, mut py) = if is_geographic(proj4) {
        (x.to_radians(), y.to_radians())
    } else {
        (x, y)
    };

    let mut point = (px, py, 0.0_f64);
    transform(&src, &wgs84, &mut point)?;
    (px, py) = (point.0, point.1);

    // proj4rs outputs radians for longlat target
    Ok((py.to_degrees(), px.to_degrees())) // (lat, lon)
}

/// Transform WGS84 lat/lon → native CRS coordinates.
/// Handles radians↔degrees conversion for geographic projections internally.
pub fn from_wgs84(lat: f64, lon: f64, proj4: &str) -> Result<(f64, f64), DemError> {
    use proj4rs::proj::Proj;
    use proj4rs::transform::transform;

    let wgs84 = Proj::from_proj_string("+proj=longlat +datum=WGS84 +no_defs")?;
    let dst = Proj::from_proj_string(proj4)?;

    // proj4rs expects radians for longlat source
    let mut point = (lon.to_radians(), lat.to_radians(), 0.0_f64);
    transform(&wgs84, &dst, &mut point)?;

    // Geographic target: proj4rs returns radians — convert back to degrees
    let (rx, ry) = if is_geographic(proj4) {
        (point.0.to_degrees(), point.1.to_degrees())
    } else {
        (point.0, point.1)
    };

    Ok((rx, ry)) // (easting/lon, northing/lat)
}

/// Look up a proj4 string for an EPSG code from the embedded crs-definitions database.
pub fn epsg_to_proj4(epsg: u32) -> Result<String, DemError> {
    let code = u16::try_from(epsg)
        .map_err(|_| DemError::from(format!("EPSG:{epsg} exceeds u16 range")))?;
    let def = crs_definitions::from_code(code)
        .ok_or_else(|| DemError::from(format!("EPSG:{epsg} not found in crs-definitions")))?;
    Ok(def.proj4.to_string())
}

/// GeoTIFF sentinel for "user-defined" projected / geographic CRS.
/// When this is the value of GeoKey 3072 or 2048, the CRS is described elsewhere
/// (a WKT in GeoAsciiParams, or inline GeoKeys + GeoDoubleParams).
const USER_DEFINED_SENTINEL: u32 = 32767;

/// Raw byte payload of the three GeoTIFF CRS tags (34735, 34736, 34737),
/// suitable for verbatim copy when writing a derived file such as the overview
/// cache. Preserving the original tags is the only way to keep the cache
/// self-describing for files whose CRS isn't a simple EPSG code (e.g. PGC's
/// HMA mosaics, which encode an Albers projection inline in the GeoKeys).
pub struct RawCrsTags {
    /// Tag 34735 (GeoKeyDirectoryTag) — raw u16 entries, upcast to u32 by tiff crate.
    pub geo_key_directory: Vec<u32>,
    /// Tag 34736 (GeoDoubleParamsTag) — empty if absent in the source.
    pub geo_double_params: Vec<f64>,
    /// Tag 34737 (GeoAsciiParamsTag) — None if absent in the source.
    pub geo_ascii_params: Option<String>,
}

/// Read all three CRS-defining GeoTIFF tags from `path`.
/// Used by the overview-cache writer to copy CRS metadata verbatim into the cache,
/// so the cache remains self-describing for any source CRS (not just EPSG-codeable).
pub fn read_raw_crs_tags(path: &Path) -> Result<RawCrsTags, DemError> {
    let file = File::open(path)?;
    let mut decoder =
        Decoder::new(std::io::BufReader::new(file)).map_err(|e| DemError::from(e.to_string()))?;

    let geo_key_directory = decoder
        .get_tag(Tag::Unknown(34735))
        .and_then(|v| v.into_u32_vec())
        .map_err(|_| DemError::from("GeoKeyDirectoryTag (34735) missing or unreadable"))?;

    let geo_double_params = decoder
        .get_tag(Tag::Unknown(34736))
        .and_then(|v| v.into_f64_vec())
        .unwrap_or_default();

    let geo_ascii_params = match decoder.get_tag(Tag::Unknown(34737)) {
        Ok(tiff::decoder::ifd::Value::Ascii(s)) => Some(s),
        _ => None,
    };

    Ok(RawCrsTags {
        geo_key_directory,
        geo_double_params,
        geo_ascii_params,
    })
}

/// Raw CRS metadata extracted from a GeoTIFF in a single file open and loop pass.
pub(crate) struct GeoKeyData {
    /// EPSG from GeoKey 3072 (projected CRS), excluding the user-defined sentinel.
    pub(crate) projected_epsg: Option<u32>,
    /// EPSG from GeoKey 2048 (geographic base CRS), excluding the user-defined sentinel.
    pub(crate) geographic_epsg: Option<u32>,
    /// WKT string from GeoKey 3073 or 2049 referencing tag 34737, if present and recognizable.
    pub(crate) wkt_candidate: Option<String>,
    /// Every GeoKey with location=0 (inline value): key_id → value.
    /// Includes both real EPSG codes and the 32767 user-defined sentinel — callers
    /// check the dedicated `projected_epsg`/`geographic_epsg` fields for "is this a
    /// real EPSG?", and check `inline.contains_key(&3075)` for the GeoKey-encoded path.
    pub(crate) inline: BTreeMap<u16, u32>,
    /// GeoDoubleParamsTag (34736) — floating-point projection parameter values.
    pub(crate) doubles: Vec<f64>,
    /// GeoKeys with location=34736: key_id → index in `doubles`.
    pub(crate) double_refs: BTreeMap<u16, u16>,
}

/// Read CRS GeoKeys from a GeoTIFF in a single file open.
///
/// Reads GeoKeyDirectoryTag (34735), GeoAsciiParamsTag (34737), and GeoDoubleParamsTag
/// (34736), then walks the GeoKey directory once to extract:
/// - inline values (location == 0): EPSG codes, projection method, units, etc.
/// - tag-34737 references (location == 34737): WKT or citation strings
/// - tag-34736 references (location == 34736): projection parameter indices
///
/// Layout of tag 34735: [KeyDirectoryVersion, KeyRevision, MinorRevision, NumberOfKeys,
/// then NumberOfKeys × 4 entries: KeyID, TIFFTagLocation, Count, ValueOffset].
pub(crate) fn read_geo_key_data(path: &Path) -> Result<GeoKeyData, DemError> {
    let file = File::open(path)?;
    let mut decoder =
        Decoder::new(std::io::BufReader::new(file)).map_err(|e| DemError::from(e.to_string()))?;
    read_geo_key_data_from_decoder(&mut decoder)
}

/// Reader-based variant of [`read_geo_key_data`]. Lets the GeoTIFF parse path read the
/// CRS GeoKeys and the image from a single decoder (one byte source) — required for the
/// in-memory (`Cursor<Vec<u8>>`) wasm path where there is no file to re-open.
pub(crate) fn read_geo_key_data_from_decoder<R: Read + Seek>(
    decoder: &mut Decoder<R>,
) -> Result<GeoKeyData, DemError> {
    let raw = decoder
        .get_tag(Tag::Unknown(34735))
        .and_then(|v| v.into_u32_vec())
        .map_err(|_| DemError::from("GeoKeyDirectoryTag (34735) missing or unreadable"))?;

    // GeoAsciiParamsTag (34737) is optional — present only in files with WKT or citation strings.
    let ascii_params: Option<String> = match decoder.get_tag(Tag::Unknown(34737)) {
        Ok(tiff::decoder::ifd::Value::Ascii(s)) => Some(s),
        _ => None,
    };

    // GeoDoubleParamsTag (34736) is optional — present only in files using the inline
    // GeoKey-encoded projection (no WKT) where parameters live in a separate doubles array.
    let doubles: Vec<f64> = decoder
        .get_tag(Tag::Unknown(34736))
        .and_then(|v| v.into_f64_vec())
        .unwrap_or_default();

    if raw.len() < 4 {
        return Err("GeoKeyDirectory too short".into());
    }

    let n_keys = raw[3] as usize;
    let mut projected_epsg: Option<u32> = None;
    let mut geographic_epsg: Option<u32> = None;
    let mut wkt_candidate: Option<String> = None;
    let mut inline: BTreeMap<u16, u32> = BTreeMap::new();
    let mut double_refs: BTreeMap<u16, u16> = BTreeMap::new();

    for i in 0..n_keys {
        let base = 4 + i * 4;
        if base + 3 >= raw.len() {
            break;
        }
        let key_id = raw[base] as u16;
        let location = raw[base + 1];
        let count = raw[base + 2] as usize;
        let value_or_offset = raw[base + 3];

        if location == 0 {
            inline.insert(key_id, value_or_offset);
            if value_or_offset != USER_DEFINED_SENTINEL {
                if key_id == 3072 {
                    projected_epsg = Some(value_or_offset);
                } else if key_id == 2048 {
                    geographic_epsg = Some(value_or_offset);
                }
            }
        }

        if location == 34736 {
            double_refs.insert(key_id, value_or_offset as u16);
        }

        // GeoKeys 3073 (PCSCitationGeoKey) or 2049 (GeogCitationGeoKey) referencing tag 34737
        if location == 34737
            && (key_id == 3073 || key_id == 2049)
            && let Some(ref ascii) = ascii_params
        {
            let offset = value_or_offset as usize;
            let end = (offset + count).min(ascii.len());
            let candidate = ascii[offset..end].trim_end_matches('\0').trim();
            if candidate.starts_with("PROJCS[")
                || candidate.starts_with("GEOGCS[")
                || candidate.starts_with("PROJCRS[")
                || candidate.starts_with("GEODCRS[")
            {
                wkt_candidate = Some(candidate.to_string());
            }
        }
    }

    Ok(GeoKeyData {
        projected_epsg,
        geographic_epsg,
        wkt_candidate,
        inline,
        doubles,
        double_refs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// HMA 8 m tile 677 (Everest): user-defined Albers Equal Area encoded as inline GeoKeys
    /// with parameters in GeoDoubleParams. Verifies the full synthesis pipeline produces
    /// a proj4 string that proj4rs can parse.
    #[test]
    fn synthesise_albers_from_inline_geokeys() {
        let mut inline = BTreeMap::new();
        inline.insert(1024, 1); // ModelType = Projected
        inline.insert(2048, 4326); // Geographic base = WGS84
        inline.insert(3072, USER_DEFINED_SENTINEL); // user-defined PCS
        inline.insert(3075, 11); // Albers
        inline.insert(3076, 9001); // metres

        let mut double_refs = BTreeMap::new();
        double_refs.insert(3078, 0); // std parallel 1 → doubles[0]
        double_refs.insert(3079, 1); // std parallel 2 → doubles[1]
        double_refs.insert(3080, 3); // lon of origin   → doubles[3]
        double_refs.insert(3081, 2); // lat of origin   → doubles[2]
        double_refs.insert(3082, 4); // false easting   → doubles[4]
        double_refs.insert(3083, 5); // false northing  → doubles[5]

        let data = GeoKeyData {
            projected_epsg: None,
            geographic_epsg: Some(4326),
            wkt_candidate: None,
            inline,
            doubles: vec![25.0, 47.0, 36.0, 85.0, 0.0, 0.0],
            double_refs,
        };

        let p4 = proj4_from_keys(&data).expect("synthesis must succeed");
        for needle in [
            "+proj=aea",
            "+lat_1=25",
            "+lat_2=47",
            "+lat_0=36",
            "+lon_0=85",
            "+x_0=0",
            "+y_0=0",
            "+datum=WGS84",
            "+units=m",
        ] {
            assert!(p4.contains(needle), "missing {needle} in: {p4}");
        }

        // Round-trip: proj4rs must parse what we synthesised, and a known point
        // (Everest summit) must transform to plausible Albers easting/northing.
        use proj4rs::proj::Proj;
        Proj::from_proj_string(&p4).expect("proj4rs must parse synthesised string");

        let (easting, northing) = from_wgs84(27.9881, 86.9250, &p4).expect("forward transform");
        // HMA tile 677 covers easting 146367..246367, northing -956211..-856211.
        // Everest at 27.99°N 86.92°E should land inside that box.
        assert!(
            (146_000.0..246_500.0).contains(&easting),
            "easting out of range: {easting}"
        );
        assert!(
            (-956_300.0..-856_100.0).contains(&northing),
            "northing out of range: {northing}"
        );
    }

    /// User-defined sentinel 32767 must never be treated as a real EPSG code,
    /// otherwise crs-definitions lookup fails for files like the HMA mosaics.
    #[test]
    fn user_defined_sentinel_excluded_from_epsg() {
        let mut inline = BTreeMap::new();
        inline.insert(3072, USER_DEFINED_SENTINEL);
        inline.insert(2048, 4326);

        let data = GeoKeyData {
            projected_epsg: None,
            geographic_epsg: Some(4326),
            wkt_candidate: None,
            inline,
            doubles: vec![],
            double_refs: BTreeMap::new(),
        };

        assert!(data.projected_epsg.is_none());
        assert_eq!(data.geographic_epsg, Some(4326));
    }

    fn empty_keys() -> GeoKeyData {
        GeoKeyData {
            projected_epsg: None,
            geographic_epsg: None,
            wkt_candidate: None,
            inline: BTreeMap::new(),
            doubles: vec![],
            double_refs: BTreeMap::new(),
        }
    }

    // is_geographic

    #[test]
    fn is_geographic_matches_longlat_and_latlong() {
        assert!(is_geographic("+proj=longlat +datum=WGS84 +no_defs"));
        assert!(is_geographic("+proj=latlong"));
        assert!(!is_geographic("+proj=utm +zone=33 +ellps=GRS80"));
        assert!(!is_geographic(""));
        // Substring match (documented quirk): any string containing "longlat" is true.
        assert!(is_geographic("longlatfoo"));
    }

    // to_wgs84 / from_wgs84

    #[test]
    fn wgs84_round_trip_through_projected_crs() {
        let laea = epsg_to_proj4(3035).unwrap();
        let (lat, lon) = (47.0, 11.0);
        let (e, n) = from_wgs84(lat, lon, &laea).unwrap();
        let (lat2, lon2) = to_wgs84(e, n, &laea).unwrap();
        assert!((lat - lat2).abs() < 1e-6, "lat {lat} vs {lat2}");
        assert!((lon - lon2).abs() < 1e-6, "lon {lon} vs {lon2}");
    }

    #[test]
    fn to_wgs84_geographic_is_identity_in_degrees() {
        let p4 = "+proj=longlat +datum=WGS84 +no_defs";
        // Input is (x=lon, y=lat) in degrees; output is (lat, lon) in degrees.
        let (lat, lon) = to_wgs84(11.0, 47.0, p4).unwrap();
        assert!((lat - 47.0).abs() < 1e-9);
        assert!((lon - 11.0).abs() < 1e-9);
    }

    #[test]
    fn to_wgs84_invalid_proj4_errors() {
        assert!(to_wgs84(0.0, 0.0, "this is not a proj string").is_err());
    }

    // epsg_to_proj4

    #[test]
    fn epsg_to_proj4_known_code() {
        assert!(epsg_to_proj4(3035).unwrap().contains("+proj=laea"));
    }

    #[test]
    fn epsg_to_proj4_above_u16_range_errors() {
        // The try_from::<u16> guard must reject codes that don't fit in u16.
        let err = epsg_to_proj4(70000).unwrap_err().to_string();
        assert!(err.contains("u16"), "expected u16-range error, got: {err}");
    }

    #[test]
    fn epsg_to_proj4_unknown_code_errors() {
        // In-range for u16 but absent from crs-definitions.
        assert!(epsg_to_proj4(65000).is_err());
    }

    // epsg_towgs84 (private, datum-shift ranges)

    #[test]
    fn epsg_towgs84_known_datums() {
        assert!(epsg_towgs84(4312).unwrap().starts_with("577.326")); // MGI
        assert!(epsg_towgs84(4314).is_some()); // DHDN
        assert!(epsg_towgs84(4277).is_some()); // OSGB36
        assert!(epsg_towgs84(4149).is_some()); // CH1903
        assert!(epsg_towgs84(4272).is_some()); // NZGD49
    }

    #[test]
    fn epsg_towgs84_range_boundaries() {
        // MGI ranges are 31254..=31259 and 31281..=31290.
        assert!(epsg_towgs84(31259).is_some(), "31259 inside first range");
        assert!(
            epsg_towgs84(31260).is_none(),
            "31260 just outside both ranges"
        );
        assert!(
            epsg_towgs84(31280).is_none(),
            "31280 just below second range"
        );
        assert!(epsg_towgs84(31281).is_some(), "31281 start of second range");
        assert!(epsg_towgs84(31290).is_some(), "31290 end of second range");
        assert!(
            epsg_towgs84(31291).is_none(),
            "31291 just above second range"
        );
    }

    #[test]
    fn epsg_towgs84_wgs84_and_unknown_have_no_shift() {
        assert!(epsg_towgs84(4326).is_none(), "WGS84 needs no datum shift");
        assert!(epsg_towgs84(99999).is_none());
    }

    // proj4_from_keys discovery paths

    #[test]
    fn proj4_from_keys_epsg_path() {
        let mut data = empty_keys();
        data.projected_epsg = Some(3035);
        let p4 = proj4_from_keys(&data).unwrap();
        assert!(p4.contains("+proj=laea"));
    }

    #[test]
    fn proj4_from_keys_injects_towgs84_for_mgi_when_absent() {
        // EPSG path for an MGI-datum projected CRS: crs-definitions provides no (or
        // zero) +towgs84, so the built-in Helmert shift must be injected. This is the
        // fix that stops the Austrian grid sitting ~600 m off the Copernicus base.
        let mut data = empty_keys();
        data.projected_epsg = Some(31287); // MGI / Austria Lambert (in 31281..=31290)
        let p4 = proj4_from_keys(&data).unwrap();
        assert!(
            p4.contains("+towgs84=577.326,90.129,463.919"),
            "MGI Helmert shift not injected: {p4}"
        );
    }

    #[test]
    fn proj4_from_keys_no_metadata_errors() {
        assert!(proj4_from_keys(&empty_keys()).is_err());
    }

    #[test]
    fn proj4_from_keys_wkt_path() {
        // A minimal WGS84 geographic WKT → proj4wkt should yield a longlat string.
        let wkt = r#"GEOGCS["WGS 84",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]]"#;
        let mut data = empty_keys();
        data.wkt_candidate = Some(wkt.to_string());
        let p4 = proj4_from_keys(&data).unwrap();
        assert!(
            is_geographic(&p4),
            "WKT path should resolve to longlat: {p4}"
        );
    }

    #[test]
    fn proj4_from_keys_inline_unsupported_transform_errors() {
        // inline path (3075 present) with an unsupported projection method code.
        let mut data = empty_keys();
        data.geographic_epsg = Some(4326);
        data.inline.insert(3075, 9999); // not a known ProjCoordTransGeoKey method
        assert!(proj4_from_keys(&data).is_err());
    }
}
