use std::fs::File;
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
/// Primary: read WKT from GeoAsciiParamsTag (34737) via GeoKey 3073 or 2049,
/// convert to proj4 using proj4wkt.
/// Fallback: look up the EPSG code from GeoKey 3072 in the crs-definitions database.
pub fn tile_proj4(path: &Path) -> Result<String, DemError> {
    proj4_from_keys(&read_geo_key_data(path)?)
}

/// Reads GeoKeyDirectoryTag (34735) and returns the EPSG code.
/// Checks ProjectedCSTypeGeoKey (3072) first, falls back to GeographicTypeGeoKey (2048).
pub fn get_tile_epsg(path: &Path) -> Result<u32, DemError> {
    read_geo_key_data(path)?
        .epsg
        .ok_or_else(|| "no CRS GeoKey (3072/2048) found in GeoKeyDirectory".into())
}

/// Resolve proj4 from already-read GeoKeyData — avoids a second file open when the
/// caller also needs the EPSG code from the same read.
pub(crate) fn proj4_from_keys(data: &GeoKeyData) -> Result<String, DemError> {
    if let Some(ref wkt) = data.wkt_candidate {
        return proj4wkt::wkt_to_projstring(wkt)
            .map_err(|e| format!("WKT found but proj4wkt failed to parse it: {e}").into());
    }
    // Fallback: EPSG code → crs-definitions
    let epsg = data
        .epsg
        .ok_or_else(|| DemError::from("no CRS GeoKey (3072/2048/3073/2049) found"))?;
    epsg_to_proj4(epsg)
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

/// Raw CRS metadata extracted from a GeoTIFF in a single file open and loop pass.
pub(crate) struct GeoKeyData {
    /// EPSG code from GeoKey 3072 (projected) or 2048 (geographic), if present.
    pub(crate) epsg: Option<u32>,
    /// WKT string from GeoKey 3073 or 2049 referencing tag 34737, if present and recognizable.
    /// Not yet converted to proj4 — callers decide whether to convert.
    pub(crate) wkt_candidate: Option<String>,
}

/// Read CRS GeoKeys from a GeoTIFF in a single file open.
///
/// Reads GeoKeyDirectoryTag (34735) and GeoAsciiParamsTag (34737), then extracts:
/// - EPSG code from inline GeoKeys 3072 or 2048 (location == 0)
/// - WKT candidate from GeoKeys 3073 or 2049 referencing tag 34737
///
/// Layout of tag 34735: [KeyDirectoryVersion, KeyRevision, MinorRevision, NumberOfKeys,
/// then NumberOfKeys × 4 entries: KeyID, TIFFTagLocation, Count, ValueOffset].
/// When TIFFTagLocation == 0, ValueOffset is the value itself.
/// When TIFFTagLocation == 34737, ValueOffset is the byte offset into tag 34737.
pub(crate) fn read_geo_key_data(path: &Path) -> Result<GeoKeyData, DemError> {
    let file = File::open(path)?;
    let mut decoder =
        Decoder::new(std::io::BufReader::new(file)).map_err(|e| DemError::from(e.to_string()))?;

    let raw = decoder
        .get_tag(Tag::Unknown(34735))
        .and_then(|v| v.into_u32_vec())
        .map_err(|_| DemError::from("GeoKeyDirectoryTag (34735) missing or unreadable"))?;

    // GeoAsciiParamsTag (34737) is optional — present only in files with WKT or citation strings.
    let ascii_params: Option<String> = match decoder.get_tag(Tag::Unknown(34737)) {
        Ok(tiff::decoder::ifd::Value::Ascii(s)) => Some(s),
        _ => None,
    };

    if raw.len() < 4 {
        return Err("GeoKeyDirectory too short".into());
    }

    let n_keys = raw[3] as usize;
    let mut epsg: Option<u32> = None;
    let mut wkt_candidate: Option<String> = None;

    for i in 0..n_keys {
        let base = 4 + i * 4;
        if base + 3 >= raw.len() {
            break;
        }
        let key_id = raw[base];
        let location = raw[base + 1];
        let count = raw[base + 2] as usize;
        let value_or_offset = raw[base + 3];

        if location == 0 && (key_id == 3072 || key_id == 2048) {
            epsg = Some(value_or_offset);
        }

        // GeoKeys 3073 (PCSCitationGeoKey) or 2049 (GeogCitationGeoKey) referencing tag 34737
        if location == 34737 && (key_id == 3073 || key_id == 2049) {
            if let Some(ref ascii) = ascii_params {
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
    }

    Ok(GeoKeyData {
        epsg,
        wkt_candidate,
    })
}
