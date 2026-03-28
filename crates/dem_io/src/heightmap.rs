use crate::DemError;
use std::{collections::HashMap, path::Path};

#[derive(Debug)]
pub struct Heightmap {
    pub data: Vec<i16>,
    pub rows: usize,
    pub cols: usize,
    pub nodata: i16,
    pub origin_lat: f64, // latitude of row 0 (north edge)
    pub origin_lon: f64, // longitude of col 0 (west edge)
    pub dx_deg: f64,     // degrees per column (east = positive)
    pub dy_deg: f64,     // degrees per row (south = negative, from .blw)
    pub dx_meters: f64,  // real-world cell width (for normals in Phase 2)
    pub dy_meters: f64,  // real-world cell height (for normals in Phase 2)
}

#[derive(Debug)]
struct HdrMeta {
    rows: usize,
    cols: usize,
    little_endian: bool, // true if BYTEORDER = I, false if M
    nodata: i16,
    origin_lon: f64, // ULXMAP
    origin_lat: f64, // ULYMAP
    x_dim: f64,      // XDIM
    y_dim: f64,      // YDIM (positive)
}

fn parse_hdr(hdr_path: &Path) -> Result<HdrMeta, DemError> {
    let hdr_content = std::fs::read_to_string(hdr_path)?;
    let lines = hdr_content.lines();

    let mut values: HashMap<&str, &str> = HashMap::new();

    for line in lines {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }

        values.insert(parts[0], parts[1]);
    }

    Ok(HdrMeta {
        rows: values
            .get("NROWS")
            .ok_or("NROWS missing in .hdr")?
            .parse()?,
        cols: values
            .get("NCOLS")
            .ok_or("NCOLS missing in .hdr")?
            .parse()?,
        little_endian: *values.get("BYTEORDER").ok_or("BYTEORDER missing in .hdr")? == "I",
        nodata: values
            .get("NODATA")
            .ok_or("NODATA missing in .hdr")?
            .parse()?,
        origin_lon: values
            .get("ULXMAP")
            .ok_or("ULXMAP missing in .hdr")?
            .parse()?,
        origin_lat: values
            .get("ULYMAP")
            .ok_or("ULYMAP missing in .hdr")?
            .parse()?,
        x_dim: values.get("XDIM").ok_or("XDIM missing in .hdr")?.parse()?,
        y_dim: values.get("YDIM").ok_or("YDIM missing in .hdr")?.parse()?,
    })
}

fn get_value_from_neighbours(
    data: &[i16],
    current_row: usize,
    current_col: usize,
    rows: usize,
    cols: usize,
    nodata: i16,
) -> i16 {
    let mut neighbours: Vec<i16> = Vec::new();

    let mut up_searchin_row = current_row;
    while up_searchin_row > 0 {
        up_searchin_row -= 1;
        let upper_cell = up_searchin_row * cols + current_col;

        if data[upper_cell] != nodata {
            neighbours.push(data[upper_cell]);
            break; // ← stop, we found the nearest valid cell
        }
    }

    let mut down_searchin_row = current_row;
    while down_searchin_row < rows - 1 {
        down_searchin_row += 1;
        let lower_cell = down_searchin_row * cols + current_col;

        if data[lower_cell] != nodata {
            neighbours.push(data[lower_cell]);
            break; // ← stop, we found the nearest valid cell
        }
    }

    let mut left_searchin_col = current_col;
    while left_searchin_col > 0 {
        left_searchin_col -= 1;
        let left_cell = current_row * cols + left_searchin_col;

        if data[left_cell] != nodata {
            neighbours.push(data[left_cell]);
            break; // ← stop, we found the nearest valid cell
        }
    }

    let mut right_searchin_col = current_col;
    while right_searchin_col < cols - 1 {
        right_searchin_col += 1;
        let right_cell = current_row * cols + right_searchin_col;

        if data[right_cell] != nodata {
            neighbours.push(data[right_cell]);
            break; // ← stop, we found the nearest valid cell
        }
    }

    let sum: i32 = neighbours.iter().map(|el| *el as i32).sum();
    let count: i32 = neighbours.len() as i32;

    // this if condition is ignored for now since it doesn't normally happens in the mountains
    // if count == 0 {
    //     // return something if no cells found
    // }

    (sum / count) as i16
}

fn fill_nodata(data: &mut [i16], rows: usize, cols: usize, nodata: i16) {
    for r in 0..rows {
        for c in 0..cols {
            let index = r * cols + c;
            if data[index] == nodata {
                let replacing = get_value_from_neighbours(data, r, c, rows, cols, nodata);
                data[index] = replacing;
            }
        }
    }
}

fn build_grayscale_png(heightmap: &Heightmap, cols: usize, rows: usize) {
    let min = *heightmap.data.iter().min().unwrap() as f32;
    let max = *heightmap.data.iter().max().unwrap() as f32;

    let pixels: Vec<u8> = heightmap
        .data
        .iter()
        .map(|&e| {
            let e = e as f32;
            ((e - min) / (max - min) * 255.0) as u8
        })
        .collect();

    image::GrayImage::from_raw(cols as u32, rows as u32, pixels)
        .unwrap()
        .save("artifacts/heightmap.png")
        .unwrap();
}

pub fn parse_bil(bil_path: &Path) -> Result<Heightmap, DemError> {
    let hdr_path = bil_path.with_extension("hdr");
    let hdr_map = parse_hdr(&hdr_path)?;
    println!("hdr_map: {:?}", hdr_map);

    let bil_bytes = std::fs::read(bil_path)?;
    let expected_size = hdr_map.rows * hdr_map.cols * 2;
    if bil_bytes.len() != expected_size {
        return Err(format!(
            "size mismatch; expected: {}; got: {};",
            expected_size,
            bil_bytes.len()
        )
        .into());
    }

    // convert to Vec<i16>
    let mut bil_data: Vec<i16> = bil_bytes
        .chunks_exact(2)
        .map(|chunk| {
            // The unwrap() is safe here because chunks_exact(2) guarantees every chunk is exactly 2 bytes — the compiler
            // just can't prove that statically, so try_into returns a Result.
            let arr: [u8; 2] = chunk.try_into().unwrap();
            if hdr_map.little_endian {
                i16::from_le_bytes(arr)
            } else {
                i16::from_be_bytes(arr)
            }
        })
        .collect();

    drop(bil_bytes);

    let before = bil_data.iter().filter(|&&v| v == hdr_map.nodata).count();
    fill_nodata(&mut bil_data, hdr_map.rows, hdr_map.cols, hdr_map.nodata);
    let after = bil_data.iter().filter(|&&v| v == hdr_map.nodata).count();
    println!("nodata cells — before: {}, after: {}", before, after);

    let min = bil_data
        .iter()
        .filter(|&&v| v != hdr_map.nodata)
        .copied()
        .min();
    let max = bil_data
        .iter()
        .filter(|&&v| v != hdr_map.nodata)
        .copied()
        .max();
    println!("elevation range check: {:?} to {:?} metres", min, max);

    let dx_deg = hdr_map.x_dim;
    let dy_deg = -hdr_map.y_dim;
    let dy_meters = hdr_map.y_dim * 111_320.0;
    let dx_meters = hdr_map.x_dim * 111_320.0 * hdr_map.origin_lat.to_radians().cos();

    let heightmap: Heightmap = Heightmap {
        data: bil_data,
        rows: hdr_map.rows,
        cols: hdr_map.cols,
        nodata: hdr_map.nodata,
        origin_lat: hdr_map.origin_lat,
        origin_lon: hdr_map.origin_lon,
        dx_deg,
        dy_deg,
        dx_meters,
        dy_meters,
    };

    build_grayscale_png(&heightmap, hdr_map.cols, hdr_map.rows);

    Ok(heightmap)
}
