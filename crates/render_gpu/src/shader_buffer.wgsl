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
// heightmap
@group(0) @binding(1) var<storage, read> hm: array<f32>;
// output
@group(0) @binding(2) var<storage, read_write> output: array<u32>;
// normals map
@group(0) @binding(3) var<storage, read> nx: array<f32>;
@group(0) @binding(4) var<storage, read> ny: array<f32>;
@group(0) @binding(5) var<storage, read> nz: array<f32>;
// shadows mask
@group(0) @binding(6) var<storage, read> shadow: array<f32>;

// bilinear height sample from storage buffer
fn sample_hm(x: f32, y: f32) -> f32 {
    let col_f = x / cam.dx_meters;
    let row_f = y / cam.dy_meters;
    let col0 = i32(col_f);
    let row0 = i32(row_f);
    let fx = col_f - f32(col0);
    let fy = row_f - f32(row0);
    let h00 = hm[u32(row0) * cam.hm_cols + u32(col0)];
    let h10 = hm[u32(row0) * cam.hm_cols + u32(col0 + 1)];
    let h01 = hm[u32(row0 + 1) * cam.hm_cols + u32(col0)];
    let h11 = hm[u32(row0 + 1) * cam.hm_cols + u32(col0 + 1)];
    return mix(mix(h00, h10, fx), mix(h01, h11, fx), fy);
}

fn binary_search_hit(t_lo_in: f32, t_hi_in: f32, dir: vec3<f32>, iterations: i32) -> vec3<f32> {
    var t_lo = t_lo_in;
    var t_hi = t_hi_in;
    for (var i = 0; i < iterations; i++) {
        let t_mid = (t_lo + t_hi) * 0.5;
        let p_mid = cam.origin + dir * t_mid;
        let h_mid = sample_hm(p_mid.x, p_mid.y);
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
    if gid.x >= cam.img_width || gid.y >= cam.img_height {
        return;
    }

    let u = (f32(gid.x) + 0.5) / f32(cam.img_width);
    let v = (f32(gid.y) + 0.5) / f32(cam.img_height);
    let ndc_x = -(2.0 * u - 1.0);
    let ndc_y = 1.0 - 2.0 * v;
    let dir = normalize(cam.forward + cam.right * ndc_x * cam.half_w + cam.up * ndc_y * cam.half_h);

    var pos = cam.origin;
    var t = 0.0;
    var hit = false;

    loop {
        let col = i32(pos.x / cam.dx_meters);
        let row = i32(pos.y / cam.dy_meters);

        if col < 0 || row < 0 || col >= i32(cam.hm_cols) || row >= i32(cam.hm_rows) { break; }
        if t > cam.t_max { break; }

        let h = sample_hm(pos.x, pos.y);

        if pos.z <= h {
            hit = true;
            pos = binary_search_hit(t - cam.step_m, t, dir, 8);
            break;
        }

        t += cam.step_m;
        pos = cam.origin + dir * t;
    }

    if hit {
        let col_f = pos.x / cam.dx_meters;
        let row_f = pos.y / cam.dy_meters;
        let col0 = i32(col_f);
        let row0 = i32(row_f);
        let fx = col_f - f32(col0);
        let fy = row_f - f32(row0);

        let i00 = u32(row0) * cam.hm_cols + u32(col0);
        let i10 = u32(row0) * cam.hm_cols + u32(col0 + 1);
        let i01 = u32(row0 + 1) * cam.hm_cols + u32(col0);
        let i11 = u32(row0 + 1) * cam.hm_cols + u32(col0 + 1);

        let n00 = vec3<f32>(nx[i00], ny[i00], nz[i00]);
        let n10 = vec3<f32>(nx[i10], ny[i10], nz[i10]);
        let n01 = vec3<f32>(nx[i01], ny[i01], nz[i01]);
        let n11 = vec3<f32>(nx[i11], ny[i11], nz[i11]);
        let normal = normalize(mix(mix(n00, n10, fx), mix(n01, n11, fx), fy));

        let normalized_sun_dir = normalize(cam.sun_dir);
        let diffuse = max(0.0, dot(normal, normalized_sun_dir));

        let ambient = 0.15;
        let s00 = shadow[i00];
        let s10 = shadow[i10];
        let s01 = shadow[i01];
        let s11 = shadow[i11];
        let in_shadow = mix(mix(s00, s10, fx), mix(s01, s11, fx), fy);
        let shadow_factor: f32 = 0.5 + 0.5 * in_shadow;
        let brightness = (ambient + (1.0 - ambient) * diffuse) * shadow_factor;

        let green       = vec3<f32>(120.0, 160.0,  80.0);
        let light_green = vec3<f32>(160.0, 175.0, 130.0);
        let rock        = vec3<f32>(200.0, 200.0, 195.0);
        let snow        = vec3<f32>(240.0, 245.0, 250.0);

        let t1 = smoothstep(1800.0, 2000.0, pos.z);
        let t2 = smoothstep(2000.0, 2200.0, pos.z);
        let t3 = smoothstep(2600.0, 2800.0, pos.z);
        let base = mix(mix(mix(green, light_green, t1), rock, t2), snow, t3);

        let r = u32(clamp(base.x * brightness, 0.0, 255.0));
        let g = u32(clamp(base.y * brightness, 0.0, 255.0));
        let b = u32(clamp(base.z * brightness, 0.0, 255.0));

        let sky = vec3<f32>(135.0, 206.0, 235.0);
        let fog_t = smoothstep(15000.0, 60000.0, t);
        let fr = mix(f32(r), sky.x, fog_t);
        let fg = mix(f32(g), sky.y, fog_t);
        let fb = mix(f32(b), sky.z, fog_t);

        output[gid.y * cam.img_width + gid.x] = u32(fr) | (u32(fg) << 8u) | (u32(fb) << 16u) | (255u << 24u);
    } else {
        output[gid.y * cam.img_width + gid.x] = 135u | (206u << 8u) | (235u << 16u) | (255u << 24u);
    }
}
