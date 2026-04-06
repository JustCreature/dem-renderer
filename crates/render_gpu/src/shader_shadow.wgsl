struct ShadowUniforms {
    hm_cols:   u32,
    hm_rows:   u32,
    dx_meters: f32,
    tan_sun:   f32,
}

@group(0) @binding(0) var<uniform> u: ShadowUniforms;
@group(0) @binding(1) var hm_tex: texture_2d<f32>;
@group(0) @binding(2) var hm_sampler: sampler;
@group(0) @binding(3) var<storage, read_write> shadow: array<f32>;

// One thread per row — sweep left to right, propagating running max.
// Rows are fully independent of each other → embarrassingly parallel at row level.
// Within each row there is a serial dependency (running_max), so each thread
// executes a sequential loop over all columns.
@compute
@workgroup_size(64, 1, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let row = gid.x;
    if row >= u.hm_rows { return; }

    let step = u.dx_meters * u.tan_sun;  // height gain per column step at this sun elevation

    var running_max: f32 = -3.40282347e+38;  // f32::NEG_INFINITY

    for (var c: u32 = 0u; c < u.hm_cols; c++) {
        let h     = textureLoad(hm_tex, vec2<i32>(i32(c), i32(row)), 0).r;
        let h_eff = h + f32(c) * step;

        // shadow[...] was initialised to 1.0 on the CPU side; only overwrite in-shadow pixels
        if h_eff < running_max {
            shadow[row * u.hm_cols + c] = 0.0;
        }

        running_max = max(running_max, h_eff);
    }
}
