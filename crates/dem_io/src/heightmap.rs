use crate::DemError;
use std::{collections::HashMap, path::Path};

#[derive(Debug)]
pub struct Heightmap {
    pub data: Vec<f32>,
    pub rows: usize,
    pub cols: usize,
    pub nodata: f32,
    pub origin_lat: f64, // latitude of row 0 (north edge)
    pub origin_lon: f64, // longitude of col 0 (west edge)
    pub dx_deg: f64,     // degrees per column (east = positive)
    pub dy_deg: f64,     // degrees per row (south = negative, from .blw)
    pub dx_meters: f64,  // real-world cell width (for normals in Phase 2)
    pub dy_meters: f64,  // real-world cell height (for normals in Phase 2)
    /// Raw tiepoint from the file in its native CRS units.
    /// Geographic (EPSG:4326): same as origin_lon / origin_lat (degrees).
    /// Projected (EPSG:31287): easting / northing of the top-left corner (metres).
    pub crs_origin_x: f64,
    pub crs_origin_y: f64,
    pub crs_epsg: u32,
    pub crs_proj4: String, // proj4 string for the tile's native CRS
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
    data: &[f32],
    current_row: usize,
    current_col: usize,
    rows: usize,
    cols: usize,
    nodata: f32,
) -> f32 {
    let mut neighbours: Vec<f32> = Vec::new();

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

    let sum: f32 = neighbours.iter().sum();
    let count = neighbours.len() as f32;

    // this if condition is ignored for now since it doesn't normally happens in the mountains
    // if count == 0 {
    //     // return something if no cells found
    // }

    sum / count
}

pub(crate) fn fill_nodata(data: &mut [f32], rows: usize, cols: usize, nodata: f32) {
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

/// Replace every cell whose height is below the `-1000` NoData sentinel with sea level
/// (0.0). The interim policy for sub-radius / coastal datasets where the source
/// genuinely has no terrain over ocean (NOAA Oahu LiDAR is the motivating case): we
/// would rather see a flat sea than a 9 km chasm. See `docs/improvements/nodata_policy.md`
/// for the design discussion and the planned `NodataPolicy` enum that will eventually
/// let the caller choose between sea-clamp, neighbour-fill, and leave-sentinel.
pub fn clamp_nodata_to_sea(hm: &mut Heightmap) {
    for v in hm.data.iter_mut() {
        if *v < -1000.0 {
            *v = 0.0;
        }
    }
}

/// Fill every cell in `hm` with height < -1000 (extract_window NODATA sentinel) by
/// sampling `base` at the corresponding world position, then smoothing the transition
/// in two passes:
///
/// 1. **Contact blend** – filled cells directly adjacent to valid data are pulled
///    50 % toward the average of their valid neighbours, so the seam value is the
///    mean of the 5m edge and the 30m fill.
/// 2. **Outward blend** – valid cells within `VALID_BLEND` pixels of the nodata
///    border are pulled toward the base value, so both sides converge to the same
///    target and the normal discontinuity disappears.
///
/// Fill uses a fast bilinear-corner approximation: 4 proj4 transforms for the window
/// corners, then bilinear interpolation of corner base-pixel coords for every cell.
pub fn fill_nodata_from_base(hm: &mut Heightmap, base: &Heightmap) {
    use crate::crs;
    use std::collections::VecDeque;

    if hm.rows == 0 || hm.cols == 0 || base.rows == 0 || base.cols == 0 {
        return;
    }

    let n = hm.rows * hm.cols;

    // Track which cells were originally NODATA so valid terrain is never modified.
    let was_nodata: Vec<bool> = hm.data.iter().map(|&h| h <= -1000.0).collect();

    // --- Fill from base (bilinear corner approximation) ---
    let is_hm_geo = crs::is_geographic(&hm.crs_proj4);
    let is_base_geo = crs::is_geographic(&base.crs_proj4);

    let hm_pixel_to_base = |r: f64, c: f64| -> Option<(f64, f64)> {
        let (hm_x, hm_y) = if is_hm_geo {
            (
                hm.crs_origin_x + c * hm.dx_deg,
                hm.crs_origin_y - r * hm.dy_deg,
            )
        } else {
            (
                hm.crs_origin_x + c * hm.dx_meters,
                hm.crs_origin_y - r * hm.dy_meters,
            )
        };
        let (lat, lon) = if is_hm_geo {
            (hm_y, hm_x)
        } else {
            crs::to_wgs84(hm_x, hm_y, &hm.crs_proj4).ok()?
        };
        let (base_x, base_y) = if is_base_geo {
            (lon, lat)
        } else {
            crs::from_wgs84(lat, lon, &base.crs_proj4).ok()?
        };
        let bc = if is_base_geo {
            (base_x - base.crs_origin_x) / base.dx_deg
        } else {
            (base_x - base.crs_origin_x) / base.dx_meters
        };
        let br = if is_base_geo {
            (base.crs_origin_y - base_y) / base.dy_deg
        } else {
            (base.crs_origin_y - base_y) / base.dy_meters
        };
        Some((bc, br))
    };

    let rc = (hm.rows - 1) as f64;
    let cc = (hm.cols - 1) as f64;
    let Some((bc00, br00)) = hm_pixel_to_base(0.0, 0.0) else {
        return;
    };
    let Some((bc01, br01)) = hm_pixel_to_base(0.0, cc) else {
        return;
    };
    let Some((bc10, br10)) = hm_pixel_to_base(rc, 0.0) else {
        return;
    };
    let Some((bc11, br11)) = hm_pixel_to_base(rc, cc) else {
        return;
    };

    let get_base = |br: i64, bc: i64| -> f32 {
        if br < 0 || br >= base.rows as i64 || bc < 0 || bc >= base.cols as i64 {
            return -9999.0;
        }
        base.data[br as usize * base.cols + bc as usize]
    };

    for r in 0..hm.rows {
        let tr = r as f64 / rc.max(1.0);
        let base_row_left = (1.0 - tr) * br00 + tr * br10;
        let base_row_right = (1.0 - tr) * br01 + tr * br11;
        let base_col_left = (1.0 - tr) * bc00 + tr * bc10;
        let base_col_right = (1.0 - tr) * bc01 + tr * bc11;

        for c in 0..hm.cols {
            let i = r * hm.cols + c;
            if !was_nodata[i] {
                continue;
            }
            let tc = c as f64 / cc.max(1.0);
            let base_col = (1.0 - tc) * base_col_left + tc * base_col_right;
            let base_row = (1.0 - tc) * base_row_left + tc * base_row_right;

            let col0 = base_col.floor() as i64;
            let row0 = base_row.floor() as i64;
            let dc = (base_col - col0 as f64) as f32;
            let dr = (base_row - row0 as f64) as f32;

            let mut ws = 0.0f32;
            let mut wt = 0.0f32;
            for (h, w) in [
                (get_base(row0, col0), (1.0 - dr) * (1.0 - dc)),
                (get_base(row0, col0 + 1), (1.0 - dr) * dc),
                (get_base(row0 + 1, col0), dr * (1.0 - dc)),
                (get_base(row0 + 1, col0 + 1), dr * dc),
            ] {
                if h > -1000.0 {
                    ws += h * w;
                    wt += w;
                }
            }
            if wt > 0.0 {
                hm.data[i] = ws / wt;
            }
        }
    }

    // --- Contact blend: pull filled boundary cells toward their valid neighbours ---
    // The fill gave each nodata cell the raw base value.  Cells right at the edge of
    // valid data would create a sharp normal discontinuity even if the height step is
    // small.  For every filled cell that touches at least one valid cell, blend 50 %
    // toward the average of those valid neighbours so the seam value sits halfway
    // between the 5m edge and the 30m fill — a single pass, no BFS propagation.
    // dist[i] = BFS distance from nearest valid-border cell (usize::MAX = unreached).
    // border_h[i] = average valid-neighbour height for the seed cells at distance 1.
    let mut dist = vec![usize::MAX; n];
    let mut border_h = vec![0.0f32; n];
    let mut queue: VecDeque<usize> = VecDeque::new();

    // Seed: filled cells that are directly adjacent to at least one valid cell.
    for r in 0..hm.rows {
        for c in 0..hm.cols {
            let i = r * hm.cols + c;
            if !was_nodata[i] || hm.data[i] <= -1000.0 {
                continue;
            }
            let mut sum_h = 0.0f32;
            let mut cnt = 0u32;
            for &(dr, dc) in &[(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                let nr = r as i32 + dr;
                let nc = c as i32 + dc;
                if nr < 0 || nr >= hm.rows as i32 || nc < 0 || nc >= hm.cols as i32 {
                    continue;
                }
                let ni = nr as usize * hm.cols + nc as usize;
                if !was_nodata[ni] {
                    sum_h += hm.data[ni];
                    cnt += 1;
                }
            }
            if cnt > 0 {
                dist[i] = 1;
                border_h[i] = sum_h / cnt as f32;
                queue.push_back(i);
            }
        }
    }

    // --- Outward blend: pull valid cells near the nodata border toward the base value ---
    // Valid cells right at the boundary keep their original sharp 5m values without this
    // step, so the seam is visible as a normal discontinuity even when the height step is
    // small.  Blending VALID_BLEND valid pixels toward the same base target makes both
    // sides of the seam converge to the same surface.
    const VALID_BLEND: usize = 30;

    let mut dist_out = vec![usize::MAX; n];
    let mut queue_out: VecDeque<usize> = VecDeque::new();

    // Seed: valid cells that touch at least one nodata cell.
    for r in 0..hm.rows {
        for c in 0..hm.cols {
            let i = r * hm.cols + c;
            if was_nodata[i] {
                continue;
            }
            let mut near_nodata = false;
            for &(dr, dc) in &[(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                let nr = r as i32 + dr;
                let nc = c as i32 + dc;
                if nr < 0 || nr >= hm.rows as i32 || nc < 0 || nc >= hm.cols as i32 {
                    continue;
                }
                if was_nodata[nr as usize * hm.cols + nc as usize] {
                    near_nodata = true;
                    break;
                }
            }
            if near_nodata {
                dist_out[i] = 1;
                queue_out.push_back(i);
            }
        }
    }

    // BFS outward through valid cells up to VALID_BLEND.
    while let Some(i) = queue_out.pop_front() {
        let d = dist_out[i];
        if d >= VALID_BLEND {
            continue;
        }
        let r = i / hm.cols;
        let c = i % hm.cols;
        for &(dr, dc) in &[(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
            let nr = r as i32 + dr;
            let nc = c as i32 + dc;
            if nr < 0 || nr >= hm.rows as i32 || nc < 0 || nc >= hm.cols as i32 {
                continue;
            }
            let ni = nr as usize * hm.cols + nc as usize;
            if was_nodata[ni] || dist_out[ni] != usize::MAX {
                continue;
            }
            dist_out[ni] = d + 1;
            queue_out.push_back(ni);
        }
    }

    // Apply: at d=1 (right at boundary) blend 75 % toward base; at d=VALID_BLEND → 0 %.
    for r in 0..hm.rows {
        let tr = r as f64 / rc.max(1.0);
        let base_row_left = (1.0 - tr) * br00 + tr * br10;
        let base_row_right = (1.0 - tr) * br01 + tr * br11;
        let base_col_left = (1.0 - tr) * bc00 + tr * bc10;
        let base_col_right = (1.0 - tr) * bc01 + tr * bc11;

        for c in 0..hm.cols {
            let i = r * hm.cols + c;
            if was_nodata[i] {
                continue;
            }
            let d = dist_out[i];
            if d == usize::MAX {
                continue;
            }
            let t = (VALID_BLEND - d) as f32 / VALID_BLEND as f32;
            let tc = c as f64 / cc.max(1.0);
            let base_col = (1.0 - tc) * base_col_left + tc * base_col_right;
            let base_row = (1.0 - tc) * base_row_left + tc * base_row_right;
            let col0 = base_col.floor() as i64;
            let row0 = base_row.floor() as i64;
            let dc = (base_col - col0 as f64) as f32;
            let dr = (base_row - row0 as f64) as f32;
            let mut ws = 0.0f32;
            let mut wt = 0.0f32;
            for (h, w) in [
                (get_base(row0, col0), (1.0 - dr) * (1.0 - dc)),
                (get_base(row0, col0 + 1), (1.0 - dr) * dc),
                (get_base(row0 + 1, col0), dr * (1.0 - dc)),
                (get_base(row0 + 1, col0 + 1), dr * dc),
            ] {
                if h > -1000.0 {
                    ws += h * w;
                    wt += w;
                }
            }
            if wt > 0.0 {
                hm.data[i] = hm.data[i] * (1.0 - t) + (ws / wt) * t;
            }
        }
    }
}

fn build_grayscale_png(heightmap: &Heightmap, cols: usize, rows: usize) {
    let min = heightmap.data.iter().cloned().fold(f32::INFINITY, f32::min);
    let max = heightmap
        .data
        .iter()
        .cloned()
        .fold(f32::NEG_INFINITY, f32::max);

    let pixels: Vec<u8> = heightmap
        .data
        .iter()
        .map(|&e| ((e - min) / (max - min) * 255.0) as u8)
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

    const NODATA_F32: f32 = -9999.0;

    // Parse i16 bytes and convert immediately to f32; map the i16 nodata sentinel to NODATA_F32.
    let mut bil_data: Vec<f32> = bil_bytes
        .chunks_exact(2)
        .map(|chunk| {
            // The unwrap() is safe here because chunks_exact(2) guarantees every chunk is exactly 2 bytes — the compiler
            // just can't prove that statically, so try_into returns a Result.
            let arr: [u8; 2] = chunk.try_into().unwrap();
            let raw = if hdr_map.little_endian {
                i16::from_le_bytes(arr)
            } else {
                i16::from_be_bytes(arr)
            };
            if raw == hdr_map.nodata {
                NODATA_F32
            } else {
                raw as f32
            }
        })
        .collect();

    drop(bil_bytes);

    let before = bil_data.iter().filter(|&&v| v == NODATA_F32).count();
    fill_nodata(&mut bil_data, hdr_map.rows, hdr_map.cols, NODATA_F32);
    let after = bil_data.iter().filter(|&&v| v == NODATA_F32).count();
    println!("nodata cells — before: {}, after: {}", before, after);

    let min = bil_data
        .iter()
        .cloned()
        .filter(|&v| v != NODATA_F32)
        .fold(f32::INFINITY, f32::min);
    let max = bil_data
        .iter()
        .cloned()
        .filter(|&v| v != NODATA_F32)
        .fold(f32::NEG_INFINITY, f32::max);
    println!("elevation range check: {} to {} metres", min, max);

    let dx_deg = hdr_map.x_dim;
    let dy_deg = -hdr_map.y_dim;
    let dy_meters = hdr_map.y_dim * 111_320.0;
    let dx_meters = hdr_map.x_dim * 111_320.0 * hdr_map.origin_lat.to_radians().cos();

    let heightmap: Heightmap = Heightmap {
        data: bil_data,
        rows: hdr_map.rows,
        cols: hdr_map.cols,
        nodata: NODATA_F32,
        origin_lat: hdr_map.origin_lat,
        origin_lon: hdr_map.origin_lon,
        dx_deg,
        dy_deg,
        dx_meters,
        dy_meters,
        crs_origin_x: hdr_map.origin_lon,
        crs_origin_y: hdr_map.origin_lat,
        crs_epsg: 4326,
        crs_proj4: "+proj=longlat +datum=WGS84 +no_defs".to_string(),
    };

    build_grayscale_png(&heightmap, hdr_map.cols, hdr_map.rows);

    Ok(heightmap)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal projected heightmap (1 m/px, sentinel −9999). dx_deg = 0 marks it
    /// projected for the code paths that branch on that.
    fn mk_hm(rows: usize, cols: usize, data: Vec<f32>) -> Heightmap {
        assert_eq!(data.len(), rows * cols, "mk_hm: data length mismatch");
        Heightmap {
            data,
            rows,
            cols,
            nodata: -9999.0,
            origin_lat: 0.0,
            origin_lon: 0.0,
            dx_deg: 0.0,
            dy_deg: 0.0,
            dx_meters: 1.0,
            dy_meters: 1.0,
            crs_origin_x: 0.0,
            crs_origin_y: 0.0,
            crs_epsg: 0,
            crs_proj4: String::new(),
        }
    }

    /// Geographic heightmap with a top-left (lon, lat) origin and 1°/px spacing.
    /// `proj4` carries "longlat" so `is_geographic` is true and `fill_nodata_from_base`
    /// stays in its arithmetic-only branch (no proj4rs transform → deterministic).
    fn mk_geo(rows: usize, cols: usize, lon0: f64, lat0: f64, data: Vec<f32>) -> Heightmap {
        let mut hm = mk_hm(rows, cols, data);
        hm.dx_deg = 1.0;
        hm.dy_deg = 1.0;
        hm.crs_origin_x = lon0;
        hm.crs_origin_y = lat0;
        hm.origin_lon = lon0;
        hm.origin_lat = lat0;
        hm.crs_proj4 = "+proj=longlat +datum=WGS84 +no_defs".to_string();
        hm
    }

    // ----- fill_nodata -----------------------------------------------------

    #[test]
    fn fill_nodata_fills_with_mean_of_nearest_valid_along_rays() {
        // Center is nodata; its four cardinal neighbours are 10/20/30/40.
        // get_value_from_neighbours averages the nearest valid cell in each ray.
        let n = -9999.0;
        #[rustfmt::skip]
        let mut data = vec![
            0.0, 20.0, 0.0,
            10.0,  n,  40.0,
            0.0, 30.0, 0.0,
        ];
        // The corner zeros are valid data (0.0 is not the sentinel here).
        fill_nodata(&mut data, 3, 3, n);
        assert_eq!(data[4], (20.0 + 30.0 + 10.0 + 40.0) / 4.0);
    }

    #[test]
    fn fill_nodata_scans_past_intervening_nodata_to_next_valid() {
        // Row 0: [nodata, nodata, 7]. The cell at (0,0) finds 7 by scanning right
        // past the intervening nodata, and finds nothing up/left/down → mean = 7.
        let n = -9999.0;
        let mut data = vec![n, n, 7.0];
        fill_nodata(&mut data, 1, 3, n);
        assert_eq!(data[0], 7.0);
    }

    #[test]
    fn fill_nodata_isolated_yields_nan_documents_open_bug() {
        // KNOWN BUG (see CLAUDE.md "Open Items": fill_nodata division-by-zero if all
        // 4 directions hit a boundary without finding valid data). A 1×1 all-nodata
        // map has zero valid neighbours → 0.0/0.0 → NaN. This test PINS the current
        // (buggy) behavior; a fix that returns a sentinel/leaves the value should
        // update this assertion.
        let n = -9999.0;
        let mut data = vec![n];
        fill_nodata(&mut data, 1, 1, n);
        assert!(data[0].is_nan(), "expected NaN tripwire, got {}", data[0]);
    }

    #[test]
    fn fill_nodata_fully_nodata_region_propagates_nan() {
        // A 3×3 with no valid cell anywhere: the first cell averages an empty set
        // → NaN, and because `NaN != sentinel`, the NaN then counts as a "valid"
        // neighbour for later cells, so the whole grid turns to NaN. (Same root
        // div-by-zero bug; demonstrates it spreads, not just a single cell.)
        let n = -9999.0;
        let mut data = vec![n; 9];
        fill_nodata(&mut data, 3, 3, n);
        assert!(
            data.iter().all(|v| v.is_nan()),
            "all cells should be NaN: {data:?}"
        );
    }

    #[test]
    fn fill_nodata_all_valid_is_noop() {
        let n = -9999.0;
        let mut data = vec![1.0, 2.0, 3.0, 4.0];
        let before = data.clone();
        fill_nodata(&mut data, 2, 2, n);
        assert_eq!(data, before);
    }

    // ----- clamp_nodata_to_sea --------------------------------------------

    #[test]
    fn clamp_nodata_to_sea_uses_strict_less_than() {
        let mut hm = mk_hm(1, 4, vec![-1000.1, -1000.0, -5000.0, 42.0]);
        clamp_nodata_to_sea(&mut hm);
        assert_eq!(hm.data[0], 0.0, "below −1000 → sea");
        assert_eq!(
            hm.data[1], -1000.0,
            "exactly −1000 is NOT clamped (strict <)"
        );
        assert_eq!(hm.data[2], 0.0);
        assert_eq!(hm.data[3], 42.0, "valid terrain untouched");
    }

    #[test]
    fn clamp_nodata_to_sea_leaves_nan_unchanged() {
        // Surprising: NaN < -1000.0 is false, so NaN is never clamped to sea.
        let mut hm = mk_hm(1, 1, vec![f32::NAN]);
        clamp_nodata_to_sea(&mut hm);
        assert!(hm.data[0].is_nan(), "NaN must survive clamp");
    }

    #[test]
    fn clamp_nodata_to_sea_empty_is_noop() {
        let mut hm = mk_hm(0, 0, vec![]);
        clamp_nodata_to_sea(&mut hm);
        assert!(hm.data.is_empty());
    }

    // ----- fill_nodata_from_base ------------------------------------------

    #[test]
    fn fill_from_base_fills_fully_nodata_window() {
        // Base is a constant 100 m plateau; hm is entirely nodata over the same
        // geographic extent → every cell is filled to ~100.
        let base = mk_geo(4, 4, 0.0, 4.0, vec![100.0; 16]);
        let mut hm = mk_geo(4, 4, 0.0, 4.0, vec![-9999.0; 16]);
        fill_nodata_from_base(&mut hm, &base);
        for (i, &v) in hm.data.iter().enumerate() {
            assert!((v - 100.0).abs() < 1e-3, "cell {i} = {v}, expected ~100");
        }
    }

    #[test]
    fn fill_from_base_leaves_valid_data_untouched_when_no_nodata() {
        let base = mk_geo(4, 4, 0.0, 4.0, vec![100.0; 16]);
        let mut hm = mk_geo(4, 4, 0.0, 4.0, (0..16).map(|i| i as f32 + 200.0).collect());
        let before = hm.data.clone();
        fill_nodata_from_base(&mut hm, &base);
        assert_eq!(hm.data, before, "no nodata → no modification at all");
    }

    #[test]
    fn fill_from_base_treats_exactly_minus_1000_as_nodata() {
        // was_nodata uses `<= -1000.0` (note: clamp_nodata_to_sea uses strict `<`).
        // One row of 40 cells: col 0 is the only nodata (−1000.0, exactly on the
        // boundary). A valid −999.0 sits at col 39, far beyond the 30-px outward
        // blend reach, so it stays untouched — proving −1000.0 was treated as
        // nodata (filled from base) while −999.0 was treated as valid (kept).
        let base = mk_geo(1, 40, 0.0, 1.0, vec![55.0; 40]);
        let mut row = vec![50.0f32; 40];
        row[0] = -1000.0;
        row[39] = -999.0;
        let mut hm = mk_geo(1, 40, 0.0, 1.0, row);
        fill_nodata_from_base(&mut hm, &base);
        assert!(
            (hm.data[0] - 55.0).abs() < 1e-2,
            "−1000 cell filled from base, got {}",
            hm.data[0]
        );
        assert_eq!(hm.data[39], -999.0, "−999 (valid, far from seam) is kept");
    }

    #[test]
    fn fill_from_base_keeps_sentinel_where_base_does_not_cover() {
        // Base sits far to the east (lon 1000..) so every hm cell maps outside the
        // base raster → get_base returns −9999, wt stays 0, the cell is left as the
        // nodata sentinel rather than corrupted.
        let base = mk_geo(2, 2, 1000.0, 2.0, vec![55.0; 4]);
        let mut hm = mk_geo(2, 2, 0.0, 2.0, vec![-9999.0; 4]);
        fill_nodata_from_base(&mut hm, &base);
        for &v in &hm.data {
            assert!(v <= -1000.0, "uncovered cell kept as sentinel, got {v}");
        }
    }

    #[test]
    fn fill_from_base_empty_inputs_early_return() {
        let base = mk_geo(2, 2, 0.0, 2.0, vec![1.0; 4]);
        let mut empty = mk_geo(0, 0, 0.0, 0.0, vec![]);
        fill_nodata_from_base(&mut empty, &base); // must not panic
        assert!(empty.data.is_empty());

        let mut hm = mk_geo(2, 2, 0.0, 2.0, vec![-9999.0; 4]);
        let empty_base = mk_geo(0, 0, 0.0, 0.0, vec![]);
        fill_nodata_from_base(&mut hm, &empty_base); // empty base → early return
        for &v in &hm.data {
            assert!(v <= -1000.0, "empty base must not fill anything");
        }
    }
}
