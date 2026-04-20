use std::fs::File;
use std::path::Path;
use tiff::decoder::{Decoder, DecodingResult};
use tiff::tags::Tag;

use crate::heightmap::fill_nodata;
use crate::{DemError, Heightmap};

pub fn parse_geotiff(path: &Path) -> Result<Heightmap, DemError> {
    let file: File = File::open(path)?;
    let mut decoder: Decoder<std::io::BufReader<File>> =
        Decoder::new(std::io::BufReader::new(file))?;

    let (cols, rows): (u32, u32) = decoder.dimensions()?;

    let scale = decoder.get_tag(Tag::Unknown(33550))?.into_f64_vec()?; // ModelPixelScaleTag
    // → Value::Double([sx, sy, sz])  — sx = deg/pixel in X, sy = deg/pixel in Y

    let tiepoint = decoder.get_tag(Tag::Unknown(33922))?.into_f64_vec()?; // ModelTiepointTag
    // → Value::Double([i, j, k, x, y, z])  — x = origin_lon, y = origin_lat

    let (dx_deg, dy_deg) = (scale[0], -scale[1]);
    let (origin_lon, origin_lat) = (tiepoint[3], tiepoint[4]);

    let img = decoder.read_image()?;
    let raw: Vec<f32> = match img {
        DecodingResult::F32(v) => v,
        _ => return Err("expected F32 image".into()),
    };

    // From the earlier analysis: NaN and values < -1000 are either tile padding or voids:
    const NODATA: f32 = -9999.0;
    let mut data: Vec<f32> = raw
        .iter()
        .map(|&v| if v.is_nan() || v < -1000.0 { NODATA } else { v })
        .collect();

    let before = data.iter().filter(|&&v| v == NODATA).count();
    fill_nodata(&mut data, rows as usize, cols as usize, NODATA);
    let after = data.iter().filter(|&&v| v == NODATA).count();
    println!("nodata cells — before: {}, after: {}", before, after);

    let min = data
        .iter()
        .cloned()
        .filter(|&v| v != NODATA)
        .fold(f32::INFINITY, f32::min);
    let max = data
        .iter()
        .cloned()
        .filter(|&v| v != NODATA)
        .fold(f32::NEG_INFINITY, f32::max);
    println!("elevation range check: {} to {} metres", min, max);

    let dy_meters = scale[1] * 111_320.0;
    let dx_meters = dx_deg * 111_320.0 * origin_lat.to_radians().cos();

    let heightmap: Heightmap = Heightmap {
        data: data,
        rows: rows as usize,
        cols: cols as usize,
        nodata: NODATA,
        origin_lat: origin_lat,
        origin_lon: origin_lon,
        dx_deg,
        dy_deg,
        dx_meters,
        dy_meters,
    };

    Ok(heightmap)
}
