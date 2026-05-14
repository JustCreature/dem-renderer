use rustfft::{FftPlanner, num_complex::Complex};

use crate::{Heightmap, projection::Projection};

const MAX_GRID: usize = 512;

/// Estimate the horizontal alignment correction (dx, dy) in metres needed to align `target`
/// onto `reference`. Positive dx shifts the target east; positive dy shifts it south.
///
/// Uses FFT phase correlation on bilinearly-sampled elevation grids in the overlap region.
/// Returns `None` if the tiles do not overlap or the correlation peak is ambiguous.
pub fn estimate_alignment(
    reference: &Heightmap,
    ref_proj: &dyn Projection,
    target: &Heightmap,
    tgt_proj: &dyn Projection,
) -> Option<(f32, f32)> {
    let (r_lat_min, r_lat_max, r_lon_min, r_lon_max) = wgs84_bounds(reference, ref_proj);
    let (t_lat_min, t_lat_max, t_lon_min, t_lon_max) = wgs84_bounds(target, tgt_proj);

    let lat_min = r_lat_min.max(t_lat_min);
    let lat_max = r_lat_max.min(t_lat_max);
    let lon_min = r_lon_min.max(t_lon_min);
    let lon_max = r_lon_max.min(t_lon_max);

    if lat_min >= lat_max || lon_min >= lon_max {
        return None;
    }

    let lat_c = (lat_min + lat_max) * 0.5;
    let lon_c = (lon_min + lon_max) * 0.5;

    // Sample at the coarser of the two resolutions.
    let grid_res = reference.dx_meters.max(target.dx_meters);

    let m_per_deg_lat = 111_000.0_f64;
    let m_per_deg_lon = 111_000.0 * lat_c.to_radians().cos();
    let overlap_h_m = (lat_max - lat_min) * m_per_deg_lat;
    let overlap_w_m = (lon_max - lon_min) * m_per_deg_lon;

    let n_rows = ((overlap_h_m / grid_res) as usize)
        .min(MAX_GRID)
        .max(32)
        .next_power_of_two();
    let n_cols = ((overlap_w_m / grid_res) as usize)
        .min(MAX_GRID)
        .max(32)
        .next_power_of_two();

    // Sampling grid origin in target CRS
    let (centre_e, centre_n) = tgt_proj.forward(lat_c, lon_c);

    let mut grid_ref = vec![0.0f32; n_rows * n_cols];
    let mut grid_tgt = vec![0.0f32; n_rows * n_cols];
    let mut valid = 0usize;

    for r in 0..n_rows {
        for c in 0..n_cols {
            let e = centre_e + (c as f64 - n_cols as f64 * 0.5) * grid_res;
            let n_coord = centre_n - (r as f64 - n_rows as f64 * 0.5) * grid_res;

            let tv = sample_crs(target, e, n_coord);
            let (lat, lon) = tgt_proj.inverse(e, n_coord);
            let (re, rn) = ref_proj.forward(lat, lon);
            let rv = sample_crs(reference, re, rn);

            // Reject NaN (out-of-bounds bilinear) and NODATA sentinel (-9999.0).
            // extract_window does not call fill_nodata, so uncovered edge pixels stay
            // at -9999.0 rather than f32::NAN — the is_nan() check alone misses them.
            if !tv.is_nan() && tv > -1000.0 && !rv.is_nan() && rv > -1000.0 {
                grid_ref[r * n_cols + c] = rv;
                grid_tgt[r * n_cols + c] = tv;
                valid += 1;
            }
        }
    }

    if valid < (n_rows * n_cols) / 4 {
        return None;
    }

    // Remove DC component using only valid pixels; invalid pixels stay at 0 (neutral).
    // Computing the mean over all pixels (including 0-initialized invalids) would leave
    // them at -mean after subtraction — a large spike that corrupts the FFT.
    let inv = 1.0 / valid as f32;
    let mean_r: f32 = grid_ref.iter().zip(grid_tgt.iter())
        .filter(|(r, t)| **r != 0.0 || **t != 0.0)
        .map(|(r, _)| *r)
        .sum::<f32>() * inv;
    let mean_t: f32 = grid_ref.iter().zip(grid_tgt.iter())
        .filter(|(r, t)| **r != 0.0 || **t != 0.0)
        .map(|(_, t)| *t)
        .sum::<f32>() * inv;
    for (r, t) in grid_ref.iter_mut().zip(grid_tgt.iter_mut()) {
        if *r != 0.0 || *t != 0.0 {
            *r -= mean_r;
            *t -= mean_t;
        }
        // else: invalid pixel stays 0 — zero contribution to cross-correlation
    }

    hann_2d(&mut grid_ref, n_rows, n_cols);
    hann_2d(&mut grid_tgt, n_rows, n_cols);

    let (shift_row, shift_col) = phase_correlate(&grid_ref, &grid_tgt, n_rows, n_cols)?;

    // C[n] = Σ_m ref[m]·tgt[m−n], peak at n = d where tgt[m] = ref[m+d].
    // tgt[m] = ref[m+d] means tgt data is d pixels east/south of its claimed coords.
    // align_d* tells the shader "shift the origin by d to correct for that offset."
    let align_dx = shift_col as f32 * grid_res as f32;
    let align_dy = shift_row as f32 * grid_res as f32;

    Some((align_dx, align_dy))
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn wgs84_bounds(hm: &Heightmap, proj: &dyn Projection) -> (f64, f64, f64, f64) {
    if hm.dx_deg != 0.0 {
        let lon_min = hm.crs_origin_x;
        let lon_max = hm.crs_origin_x + hm.cols as f64 * hm.dx_deg;
        let lat_max = hm.crs_origin_y;
        let lat_min = hm.crs_origin_y - hm.rows as f64 * hm.dy_deg.abs();
        (lat_min, lat_max, lon_min, lon_max)
    } else {
        let e_min = hm.crs_origin_x;
        let e_max = hm.crs_origin_x + hm.cols as f64 * hm.dx_meters;
        let n_max = hm.crs_origin_y;
        let n_min = hm.crs_origin_y - hm.rows as f64 * hm.dy_meters;
        let mut lat_min = f64::INFINITY;
        let mut lat_max = f64::NEG_INFINITY;
        let mut lon_min = f64::INFINITY;
        let mut lon_max = f64::NEG_INFINITY;
        for (e, n) in [(e_min, n_min), (e_min, n_max), (e_max, n_min), (e_max, n_max)] {
            let (lat, lon) = proj.inverse(e, n);
            lat_min = lat_min.min(lat);
            lat_max = lat_max.max(lat);
            lon_min = lon_min.min(lon);
            lon_max = lon_max.max(lon);
        }
        (lat_min, lat_max, lon_min, lon_max)
    }
}

fn sample_crs(hm: &Heightmap, crs_e: f64, crs_n: f64) -> f32 {
    let (col_f, row_f) = if hm.dx_deg != 0.0 {
        (
            (crs_e - hm.crs_origin_x) / hm.dx_deg,
            (hm.crs_origin_y - crs_n) / hm.dy_deg.abs(),
        )
    } else {
        (
            (crs_e - hm.crs_origin_x) / hm.dx_meters,
            (hm.crs_origin_y - crs_n) / hm.dy_meters,
        )
    };

    let c0 = col_f.floor() as isize;
    let r0 = row_f.floor() as isize;
    let fc = (col_f - col_f.floor()) as f32;
    let fr = (row_f - row_f.floor()) as f32;

    let get = |r: isize, c: isize| -> f32 {
        if r < 0 || r >= hm.rows as isize || c < 0 || c >= hm.cols as isize {
            return f32::NAN;
        }
        hm.data[r as usize * hm.cols + c as usize]
    };

    let v00 = get(r0, c0);
    let v01 = get(r0, c0 + 1);
    let v10 = get(r0 + 1, c0);
    let v11 = get(r0 + 1, c0 + 1);
    if v00.is_nan() || v01.is_nan() || v10.is_nan() || v11.is_nan() {
        return f32::NAN;
    }
    v00 + fc * (v01 - v00) + fr * ((v10 + fc * (v11 - v10)) - (v00 + fc * (v01 - v00)))
}

fn hann_2d(grid: &mut [f32], rows: usize, cols: usize) {
    use std::f32::consts::PI;
    for r in 0..rows {
        let wr = 0.5 * (1.0 - (2.0 * PI * r as f32 / (rows - 1) as f32).cos());
        for c in 0..cols {
            let wc = 0.5 * (1.0 - (2.0 * PI * c as f32 / (cols - 1) as f32).cos());
            grid[r * cols + c] *= wr * wc;
        }
    }
}

/// Returns sub-pixel (shift_row, shift_col) using parabolic refinement around the peak.
fn phase_correlate(a: &[f32], b: &[f32], rows: usize, cols: usize) -> Option<(f32, f32)> {
    let mut planner = FftPlanner::<f32>::new();
    let fft_r = planner.plan_fft_forward(cols);
    let fft_c = planner.plan_fft_forward(rows);
    let ifft_r = planner.plan_fft_inverse(cols);
    let ifft_c = planner.plan_fft_inverse(rows);

    let mut fa: Vec<Complex<f32>> = a.iter().map(|&v| Complex::new(v, 0.0)).collect();
    let mut fb: Vec<Complex<f32>> = b.iter().map(|&v| Complex::new(v, 0.0)).collect();

    fft2d(&mut fa, rows, cols, &fft_r, &fft_c);
    fft2d(&mut fb, rows, cols, &fft_r, &fft_c);

    for (a, b) in fa.iter_mut().zip(fb.iter()) {
        let cross = *a * b.conj();
        let mag = cross.norm();
        *a = if mag > 1e-10 { cross / mag } else { Complex::new(0.0, 0.0) };
    }

    ifft2d(&mut fa, rows, cols, &ifft_r, &ifft_c);

    let n = (rows * cols) as f32;
    for v in &mut fa { *v /= n; }

    let (peak_idx, _) = fa
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.re.partial_cmp(&b.re).unwrap_or(std::cmp::Ordering::Equal))?;

    let pr = (peak_idx / cols) as i32;
    let pc = (peak_idx % cols) as i32;

    // Integer shift with wrap-around
    let sr = if pr > (rows / 2) as i32 { pr - rows as i32 } else { pr };
    let sc = if pc > (cols / 2) as i32 { pc - cols as i32 } else { pc };

    // Parabolic sub-pixel refinement along each axis independently.
    // Fits a parabola through (peak-1, peak, peak+1) and finds the fractional offset.
    let sub_r = {
        let rm = ((pr as usize + rows - 1) % rows) * cols + pc as usize;
        let r0 = peak_idx;
        let rp = ((pr as usize + 1) % rows) * cols + pc as usize;
        parabolic_delta(fa[rm].re, fa[r0].re, fa[rp].re)
    };
    let sub_c = {
        let cm = pr as usize * cols + (pc as usize + cols - 1) % cols;
        let c0 = peak_idx;
        let cp = pr as usize * cols + (pc as usize + 1) % cols;
        parabolic_delta(fa[cm].re, fa[c0].re, fa[cp].re)
    };

    Some((sr as f32 + sub_r, sc as f32 + sub_c))
}

/// Fractional offset of a parabola peak given three equally-spaced samples y[-1], y[0], y[1].
#[inline]
fn parabolic_delta(ym: f32, y0: f32, yp: f32) -> f32 {
    let denom = ym - 2.0 * y0 + yp;
    if denom.abs() < 1e-10 { 0.0 } else { -0.5 * (yp - ym) / denom }
}

fn fft2d(
    data: &mut [Complex<f32>],
    rows: usize,
    cols: usize,
    fft_row: &std::sync::Arc<dyn rustfft::Fft<f32>>,
    fft_col: &std::sync::Arc<dyn rustfft::Fft<f32>>,
) {
    for r in 0..rows {
        fft_row.process(&mut data[r * cols..(r + 1) * cols]);
    }
    let mut buf = vec![Complex::new(0.0f32, 0.0); rows];
    for c in 0..cols {
        for r in 0..rows { buf[r] = data[r * cols + c]; }
        fft_col.process(&mut buf);
        for r in 0..rows { data[r * cols + c] = buf[r]; }
    }
}

fn ifft2d(
    data: &mut [Complex<f32>],
    rows: usize,
    cols: usize,
    ifft_row: &std::sync::Arc<dyn rustfft::Fft<f32>>,
    ifft_col: &std::sync::Arc<dyn rustfft::Fft<f32>>,
) {
    let mut buf = vec![Complex::new(0.0f32, 0.0); rows];
    for c in 0..cols {
        for r in 0..rows { buf[r] = data[r * cols + c]; }
        ifft_col.process(&mut buf);
        for r in 0..rows { data[r * cols + c] = buf[r]; }
    }
    for r in 0..rows {
        ifft_row.process(&mut data[r * cols..(r + 1) * cols]);
    }
}
