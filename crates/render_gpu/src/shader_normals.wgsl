struct NormalsUniforms {
    hm_cols:   u32,
    hm_rows:   u32,
    dx_meters: f32,
    dy_meters: f32,
}

@group(0) @binding(0) var<uniform> u: NormalsUniforms;
@group(0) @binding(1) var hm_tex: texture_2d<f32>;
@group(0) @binding(2) var hm_sampler: sampler;
@group(0) @binding(3) var<storage, read_write> nx: array<f32>;
@group(0) @binding(4) var<storage, read_write> ny: array<f32>;
@group(0) @binding(5) var<storage, read_write> nz: array<f32>;

@compute
@workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let col = i32(gid.x);
    let row = i32(gid.y);

    if gid.x >= u.hm_cols || gid.y >= u.hm_rows {
        return;
    }

    let idx = gid.y * u.hm_cols + gid.x;

    // boundary pixels — leave as zero, matching CPU behaviour
    if col == 0 || row == 0 || col == i32(u.hm_cols) - 1 || row == i32(u.hm_rows) - 1 {
        nx[idx] = 0.0;
        ny[idx] = 0.0;
        nz[idx] = 0.0;
        return;
    }

    let left  = textureLoad(hm_tex, vec2<i32>(col - 1, row    ), 0).r;
    let right = textureLoad(hm_tex, vec2<i32>(col + 1, row    ), 0).r;
    let upper = textureLoad(hm_tex, vec2<i32>(col,     row - 1), 0).r;
    let lower = textureLoad(hm_tex, vec2<i32>(col,     row + 1), 0).r;

    // same formula as compute_normals_scalar on CPU
    let nx_val = (left - right) / (2.0 * u.dx_meters);
    let ny_val = (upper - lower) / (2.0 * u.dy_meters);
    let nz_val = 1.0;

    let len = sqrt(nx_val * nx_val + ny_val * ny_val + nz_val * nz_val);

    nx[idx] = nx_val / len;
    ny[idx] = ny_val / len;
    nz[idx] = nz_val / len;
}
