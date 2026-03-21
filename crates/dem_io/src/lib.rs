use std::{collections::HashMap, path::Path};

type DemError = Box<dyn std::error::Error>;

pub struct Heightmap {
    data: Vec<i16>,
    rows: usize,
    cols: usize,
    nodata: i16,
    origin_lat: f64,  // latitude of row 0 (north edge)
    origin_lon: f64,  // longitude of col 0 (west edge)
    dx_deg: f64,  // degrees per column (east = positive)
    dy_deg: f64,  // degrees per row (south = negative, from .blw)
    dx_meters: f64,  // real-world cell width (for normals in Phase 2)
    dy_meters: f64,  // real-world cell height (for normals in Phase 2)
}

#[derive(Debug)]
struct HdrMeta {
    rows: usize,
    cols: usize,
    little_endian: bool,  // true if BYTEORDER = I, false if M
    nodata: i16,
    origin_lon: f64,  // ULXMAP
    origin_lat: f64,  // ULYMAP
    x_dim: f64,  // XDIM
    y_dim: f64,  // YDIM (positive)
}

fn parse_hdr(hdr_path: &Path) -> Result<HdrMeta, DemError> {
    let hdr_content = std::fs::read_to_string(hdr_path)?;
    let lines = hdr_content.lines();

    let mut values: HashMap<&str, &str> = HashMap::new();

    for line in lines {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 { continue; }

        values.insert(parts[0], parts[1]);
    }

    println!("test {:?}", values);

    Ok(HdrMeta {
        rows: values.get("NROWS").ok_or("NROWS missing in .hdr")?.parse()?,
        cols: values.get("NCOLS").ok_or("NCOLS missing in .hdr")?.parse()?,
        little_endian: *values.get("BYTEORDER").ok_or("BYTEORDER missing in .hdr")? == "I",
        nodata: values.get("NODATA").ok_or("NODATA missing in .hdr")?.parse()?,
        origin_lon: values.get("ULXMAP").ok_or("ULXMAP missing in .hdr")?.parse()?,
        origin_lat: values.get("ULYMAP").ok_or("ULYMAP missing in .hdr")?.parse()?,
        x_dim: values.get("XDIM").ok_or("XDIM missing in .hdr")?.parse()?,
        y_dim: values.get("YDIM").ok_or("YDIM missing in .hdr")?.parse()?,
    })

}

fn fill_nodata(data: &mut [i16], rows: usize, cols: usize, nodata: i16) {

}

pub fn parse_bil(bil_path: &Path) -> Result<Heightmap, DemError> {
    let hdr_path = bil_path.with_extension("hdr");
    let hdr_map = parse_hdr(&hdr_path)?;
    println!("hdr_map: {:?}", hdr_map);
    todo!()
}
