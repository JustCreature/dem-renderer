struct CameraUniforms {
    origin: vec3<f32>,  // 12 bytes + 4 pad = 16
    _pad0: f32,
    forward: vec3<f32>,
    _pad1: f32,
    right: vec3<f32>,
    _pad2: f32,
    up: vec3<f32>,
    _pad3: f32,
    half_w: f32,
    half_h: f32,
    img_width: u32,
    img_height: u32,
    hm_cols: u32,
    hm_rows: u32,
    dx_meters: f32,
    step_m: f32,
    t_max: f32,
    _pad4: f32,
    _pad5: f32,
    _pad6: f32,
}

@group(0) @binding(0) var<uniform> cam: CameraUniforms;
@group(0) @binding(1) var<storage, read> hm: array<f32>;
@group(0) @binding(2) var<storage, read_write> output: array<u32>;

@compute
@workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    // gid.x = pixel column, gid.y = pixel row
    // every thread runs this independently

    // bounds check — dispatch may launch threads outside the image
    if gid.x >= cam.img_width || gid.y >= cam.img_height {
        return;
    }

    // ray direction — same math as ray_for_pixel in render_cpu
    let u = (f32(gid.x) + 0.5) / f32(cam.img_width);
    let v = (f32(gid.y) + 0.5) / f32(cam.img_height);
    let ndc_x = -(2.0 * u - 1.0); // flip horizontal
    let ndc_y = 1.0 - 2.0 * v;    // flip vertical
    let dir = normalize(cam.forward + cam.right * ndc_x * cam.half_w + cam.up * ndc_y * cam.half_h);

    // debug gradient — verify thread→pixel mapping before adding march loop
    // red increases left→right, green increases top→bottom
    let r = u32(255.0 * u);
    let g = u32(255.0 * v);
    let b = 128u;
    output[gid.y * cam.img_width + gid.x] = r | (g << 8u) | (b << 16u) | (255u << 24u);
}

