pub struct NormalMap {
    pub nx: Vec<f32>,
    pub ny: Vec<f32>,
    pub nz: Vec<f32>,
    pub rows: usize,
    pub cols: usize,
}

pub fn compute_normals_scalar(hm: &dem_io::Heightmap) -> NormalMap {
    let mut nx: Vec<f32> = vec![0.0f32; hm.rows * hm.cols];
    let mut ny: Vec<f32> = vec![0.0f32; hm.rows * hm.cols];
    let mut nz: Vec<f32> = vec![0.0f32; hm.rows * hm.cols];

    for r in 1..hm.rows - 1 {
        for c in 1..hm.cols - 1 {
            let upper: f32 = hm.data[(r - 1) * hm.cols + c] as f32;
            let lower: f32 = hm.data[(r + 1) * hm.cols + c] as f32;
            let left: f32 = hm.data[r * hm.cols + (c - 1)] as f32;
            let right: f32 = hm.data[r * hm.cols + (c + 1)] as f32;

            let single_nx: f32 = (left - right) / (2.0 * hm.dx_meters) as f32;
            let single_ny: f32 = (upper - lower) / (2.0 * hm.dy_meters) as f32;
            let single_nz: f32 = 1.0;

            let length: f32 =
                f32::sqrt(single_nx * single_nx + single_ny * single_ny + single_nz * single_nz);

            nx[r * hm.cols + c] = single_nx / length;
            ny[r * hm.cols + c] = single_ny / length;
            nz[r * hm.cols + c] = single_nz / length;
        }
    }

    NormalMap {
        nx,
        ny,
        nz,
        rows: hm.rows,
        cols: hm.cols,
    }
}
