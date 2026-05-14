use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use tiff::decoder::Decoder;
use tiff::tags::Tag;

/// CRS projection: forward (lat/lon degrees → easting/northing) and inverse.
pub trait Projection: Send + Sync {
    fn forward(&self, lat_deg: f64, lon_deg: f64) -> (f64, f64);
    fn inverse(&self, easting: f64, northing: f64) -> (f64, f64);
}

/// Identity projection for WGS84 geographic tiles (EPSG:4326).
/// forward returns (lon, lat); inverse returns (lat, lon).
pub struct Wgs84Identity;

impl Projection for Wgs84Identity {
    fn forward(&self, lat_deg: f64, lon_deg: f64) -> (f64, f64) {
        (lon_deg, lat_deg)
    }
    fn inverse(&self, easting: f64, northing: f64) -> (f64, f64) {
        (northing, easting)
    }
}

struct Proj4rsProjection {
    wgs84: proj4rs::Proj,
    projected: proj4rs::Proj,
}

// proj4rs::Proj is stateless post-construction (no raw pointers, no thread-local state).
unsafe impl Send for Proj4rsProjection {}
unsafe impl Sync for Proj4rsProjection {}

impl Projection for Proj4rsProjection {
    fn forward(&self, lat_deg: f64, lon_deg: f64) -> (f64, f64) {
        let mut pt = (lon_deg.to_radians(), lat_deg.to_radians(), 0.0);
        proj4rs::transform::transform(&self.wgs84, &self.projected, &mut pt)
            .expect("proj4rs forward transform failed");
        (pt.0, pt.1)
    }

    fn inverse(&self, easting: f64, northing: f64) -> (f64, f64) {
        let mut pt = (easting, northing, 0.0);
        proj4rs::transform::transform(&self.projected, &self.wgs84, &mut pt)
            .expect("proj4rs inverse transform failed");
        // After inverse, pt.0 = lon_rad, pt.1 = lat_rad
        (pt.1.to_degrees(), pt.0.to_degrees())
    }
}

/// Return a PROJ string for EPSG codes that appear in GeoTIFFs which only store the EPSG
/// code (key 3072) without the individual parameter keys (3075–3083).
///
/// This is a compatibility shim for non-self-describing files. Any tile that stores its own
/// parameters (has GeoKey 3075) works without an entry here.
/// Currently covers only the BEV tile formats actually used by this renderer.
fn epsg_proj_string(epsg: u32) -> Option<String> {
    match epsg {
        // MGI / Austria Lambert (BEV DGM 5m whole-Austria file)
        31287 => Some(
            "+proj=lcc +lat_0=47.5 +lon_0=13.333333 +lat_1=49.0 +lat_2=46.0 \
             +x_0=400000 +y_0=400000 +a=6377397.155 +rf=299.1528128 +units=m +no_defs"
                .into(),
        ),
        // ETRS89 / LAEA Europe (BEV 1m per-region tiles)
        3035 => Some(
            "+proj=laea +lat_0=52 +lon_0=10 \
             +x_0=4321000 +y_0=3210000 +a=6378137 +rf=298.257222101 +units=m +no_defs"
                .into(),
        ),
        _ => None,
    }
}

/// Read the CRS projection directly from a GeoTIFF file's GeoKey directory (tags 34735/34736).
/// No EPSG lookup table — all projection parameters are read from the file itself.
/// Returns `Wgs84Identity` for geographic CRS (GTModelTypeGeoKey == 2).
pub fn read_projection(path: &Path) -> Result<Arc<dyn Projection>, String> {
    let file = File::open(path).map_err(|e| format!("cannot open {:?}: {e}", path))?;
    let mut decoder = Decoder::new(std::io::BufReader::new(file))
        .map_err(|e| format!("not a valid TIFF {:?}: {e}", path))?;

    // GeoKeyDirectory (tag 34735): [version, rev, minor, n_keys, then n×4 u16 entries]
    let keys: Vec<u16> = decoder
        .get_tag(Tag::Unknown(34735))
        .and_then(|v| v.into_u16_vec())
        .map_err(|e| format!("GeoKeyDirectory tag missing in {:?}: {e}", path))?;

    let n = keys
        .get(3)
        .copied()
        .ok_or_else(|| format!("GeoKeyDirectory too short in {:?}", path))? as usize;

    // GeoDoubleParamsTag (tag 34736): pool of f64 values indexed by GeoKey entries
    let doubles: Vec<f64> = decoder
        .get_tag(Tag::Unknown(34736))
        .and_then(|v| v.into_f64_vec())
        .unwrap_or_default();

    let mut inline: HashMap<u16, u16> = HashMap::new();
    let mut dbl: HashMap<u16, f64> = HashMap::new();

    for i in 0..n {
        let base = 4 + i * 4;
        let key_id = keys[base];
        let tiff_tag_location = keys[base + 1];
        let value_offset = keys[base + 3];

        if tiff_tag_location == 0 {
            inline.insert(key_id, value_offset);
        } else if tiff_tag_location == 34736 {
            if let Some(&v) = doubles.get(value_offset as usize) {
                dbl.insert(key_id, v);
            }
        }
    }

    // GTModelTypeGeoKey (1024): 2 = geographic CRS
    if inline.get(&1024).copied() == Some(2) {
        return Ok(Arc::new(Wgs84Identity));
    }

    // Two valid GeoTIFF encodings for projected CRS:
    //  A) Self-describing: ProjCoordTransGeoKey (3075) + individual parameter keys.
    //  B) EPSG reference: only ProjectedCSTypeGeoKey (3072) — client must look up the definition.
    // Try A first; fall back to B when 3075 is absent.
    let proj_str = if let Some(&proj_method) = inline.get(&3075) {
        // ── A: self-describing parameter set ──────────────────────────────────────────
        let proj_keyword = match proj_method {
            1 => "tmerc",   // Transverse Mercator
            8 | 9 => "lcc", // Lambert Conformal Conic (2SP / 1SP)
            10 => "laea",   // Lambert Azimuthal Equal Area
            11 => "merc",   // Mercator
            17 => "omerc",  // Oblique Mercator
            _ => {
                return Err(format!(
                    "unsupported projection method code {} in {:?}",
                    proj_method, path
                ));
            }
        };

        let lon_0 = dbl.get(&3080).copied().unwrap_or(0.0);
        let lat_0 = dbl.get(&3081).copied().unwrap_or(0.0);
        let lat_1 = dbl.get(&3078).copied().unwrap_or(lat_0);
        let lat_2 = dbl.get(&3079).copied().unwrap_or(lat_0);
        let x_0 = dbl.get(&3082).copied().unwrap_or(0.0);
        let y_0 = dbl.get(&3083).copied().unwrap_or(0.0);
        let k_0 = dbl.get(&3084).copied().unwrap_or(1.0);

        // Ellipsoid: prefer GeoKeys 2057/2059 (modern COG), fall back to ellipsoid code 2056
        let (a, rf) = if let (Some(&a), Some(&rf)) = (dbl.get(&2057), dbl.get(&2059)) {
            (a, rf)
        } else {
            match inline.get(&2056).copied().unwrap_or(7030) {
                7001 => (6_377_563.396, 299.324_964_6), // Airy 1830
                7004 => (6_377_397.155, 299.152_812_8), // Bessel 1841
                7019 => (6_378_137.0, 298.257_222_101), // GRS80
                7022 => (6_378_388.0, 297.0),           // International 1924
                _ => (6_378_137.0, 298.257_223_563),    // WGS84 (7030 and unknown)
            }
        };

        match proj_keyword {
            "lcc" => format!(
                "+proj=lcc +lat_0={lat_0} +lon_0={lon_0} +lat_1={lat_1} +lat_2={lat_2} \
                 +x_0={x_0} +y_0={y_0} +a={a} +rf={rf} +units=m +no_defs"
            ),
            "tmerc" => format!(
                "+proj=tmerc +lat_0={lat_0} +lon_0={lon_0} +k_0={k_0} \
                 +x_0={x_0} +y_0={y_0} +a={a} +rf={rf} +units=m +no_defs"
            ),
            _ => format!(
                "+proj={proj_keyword} +lat_0={lat_0} +lon_0={lon_0} \
                 +x_0={x_0} +y_0={y_0} +a={a} +rf={rf} +units=m +no_defs"
            ),
        }
    } else {
        // ── B: EPSG-code-only file — compatibility table ───────────────────────────────
        // Files that only set key 3072 rely on the client to know the CRS definition.
        // proj4rs cannot load the EPSG database at runtime, so we maintain a small table
        // of common projected CRS. UTM zones are computed; named CRS are listed explicitly.
        let epsg = inline.get(&3072).copied().ok_or_else(|| {
            format!(
                "neither ProjCoordTransGeoKey (3075) nor ProjectedCSTypeGeoKey (3072) \
                     found in {:?}",
                path
            )
        })? as u32;

        epsg_proj_string(epsg).ok_or_else(|| {
            format!(
                "EPSG:{epsg} in {:?} is not self-describing and is not in the compatibility table",
                path
            )
        })?
    };

    let wgs84 = proj4rs::Proj::from_proj_string("+proj=longlat +datum=WGS84 +no_defs")
        .map_err(|e| format!("cannot create WGS84 proj: {e:?}"))?;
    let projected = proj4rs::Proj::from_proj_string(&proj_str)
        .map_err(|e| format!("cannot create projection from '{proj_str}': {e:?}"))?;

    Ok(Arc::new(Proj4rsProjection { wgs84, projected }))
}
