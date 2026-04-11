struct CameraUniforms {
    origin: vec3<f32>,  // 12 bytes + 4 pad = 16
    _pad0: f32,
    forward: vec3<f32>,
    _pad1: f32,
    right: vec3<f32>,
    _pad2: f32,
    up: vec3<f32>,
    _pad3: f32,
    sun_dir: vec3<f32>,
    _pad4: f32,
    half_w: f32,
    half_h: f32,
    img_width: u32,
    img_height: u32,
    hm_cols: u32,
    hm_rows: u32,
    dx_meters: f32,
    dy_meters: f32,
    step_m: f32,
    t_max: f32,
    _pad5: f32,
    _pad6: f32,
    _pad7: f32,
    _pad8: f32,
    _pad9: f32,
    _pad10: f32,
}

// camera uniforms struct
@group(0) @binding(0) var<uniform> cam: CameraUniforms;
// heightmap texture + sampler                                                                                                                                 
@group(0) @binding(1) var hm_tex: texture_2d<f32>;
@group(0) @binding(2) var hm_sampler: sampler;
// output
@group(0) @binding(3) var<storage, read_write> output: array<u32>;
// normals map
@group(0) @binding(4) var<storage, read> nx: array<f32>;
@group(0) @binding(5) var<storage, read> ny: array<f32>;
@group(0) @binding(6) var<storage, read> nz: array<f32>;
// shadows mask
@group(0) @binding(7) var<storage, read> shadow: array<f32>;

fn binary_search_hit(t_lo_in: f32, t_hi_in: f32, dir: vec3<f32>, iterations: i32) -> vec3<f32> {
    // binary search to refine hit position between t_prev (above) and t (below)
    // without it there will be the arc pattern and the picture will look a bit broken (glitching)
    // The arc patterns are the step-size error: 
    // all rays with the same number of steps have the same overshot distance, creating concentric iso-step contours that 
    // look like arcs in the foreground where the terrain is close and the step size is relatively large compared to surface detail.
    var t_lo = t_lo_in;
    var t_hi = t_hi_in;
    for (var i = 0; i < iterations; i++) {
        let t_mid = (t_lo + t_hi) * 0.5;
        let p_mid = cam.origin + dir * t_mid;
        let c = i32(p_mid.x / cam.dx_meters);
        let r = i32(p_mid.y / cam.dy_meters);
        let h_mid = textureLoad(hm_tex, vec2<i32>(c, r), 0).r;
        if p_mid.z <= h_mid {
            t_hi = t_mid;
        } else {
            t_lo = t_mid;
        }
    }
    return cam.origin + dir * t_lo;
}

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

    var pos = cam.origin;
    var t = 0.0;
    var t_prev = 0.0;
    var hit = false;

    loop {
        let col = i32(pos.x / cam.dx_meters);
        let row = i32(pos.y / cam.dy_meters);

        // bounds check — stop if ray left the heightmap                                                                                                           
        if col < 0 || row < 0 || col >= i32(cam.hm_cols) || row >= i32(cam.hm_rows) { break; }
        // max distance check                                                                                                                                      
        if t > cam.t_max { break; }

        let h = textureLoad(hm_tex, vec2<i32>(col, row), 0).r;

        if pos.z <= h {
            hit = true;

            // refine hit position between t_prev (above) and t (below)
            pos = binary_search_hit(t - cam.step_m, t, dir, 8);

            break;
        }

        t_prev = t;
        t += cam.step_m;
        pos = cam.origin + dir * t;
    }

    if hit {
        let col = i32(pos.x / cam.dx_meters);
        let row = i32(pos.y / cam.dy_meters);
        let idx = u32(row) * cam.hm_cols + u32(col);

        // dot(normal, sun_dir) — sun_dir must also be normalized
        let normal = vec3<f32>(nx[idx], ny[idx], nz[idx]);
        let normalized_sun_dir = normalize(cam.sun_dir);
        let diffuse = max(0.0, dot(normal, normalized_sun_dir));

        let ambient = 0.15;
        // let ambient = 0.3; // less dark shadow faces
        let in_shadow: f32 = shadow[idx];
        let shadow_factor: f32 = 0.5 + 0.5 * in_shadow;
        let brightness = (ambient + (1.0 - ambient) * diffuse) * shadow_factor;

        // let base = [180u8, 160u8, 140u8]; // rocky grey-brown
        // let base = [200u8, 200u8, 195u8]; // cool grey-white
        // WGSL requires variables to be initialised or have a default value
        var base_r: f32 = 0.0; var base_g: f32 = 0.0; var base_b: f32 = 0.0;
        if pos.z < 1900.0 {
            base_r = 120.0; base_g = 160.0; base_b = 80.0; // green                                                                                                                            
        } else if pos.z < 2100.0 {
            base_r = 160.0; base_g = 175.0; base_b = 130.0; // slightly green                                                                                                                   
        } else if pos.z < 2700.0 {
            base_r = 200.0; base_g = 200.0; base_b = 195.0; // grey-white rock                                                                                                                
        } else {
            base_r = 240.0; base_g = 245.0; base_b = 250.0; // glacier snow white                                                                                                             
        };

        let r = u32(clamp(base_r * brightness, 0.0, 255.0));
        let g = u32(clamp(base_g * brightness, 0.0, 255.0));
        let b = u32(clamp(base_b * brightness, 0.0, 255.0));
        // output[gid.y * cam.img_width + gid.x] = r | (g << 8u) | (b << 16u) | (255u << 24u);
        output[gid.y * cam.img_width + gid.x] = b | (g << 8u) | (r << 16u) | (255u << 24u);  // bgra format
    } else {
        // sky blue
        // output[gid.y * cam.img_width + gid.x] = 135u | (206u << 8u) | (235u << 16u) | (255u << 24u);
        output[gid.y * cam.img_width + gid.x] = 235u | (206u << 8u) | (135u << 16u) | (255u << 24u);  // bgra format
    }
}

