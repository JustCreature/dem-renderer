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
    ao_mode: u32,
    _pad5: f32,
    shadows_enabled: u32,
    fog_enabled: u32,
    vat_mode: u32,
    lod_mode: u32,
    // 5m close tier (extent_x == 0.0 means inactive)
    hm5m_origin_x: f32,
    hm5m_origin_y: f32,
    hm5m_extent_x: f32,
    hm5m_extent_y: f32,
    hm5m_cols: u32,
    hm5m_rows: u32,
    _pad6: u32,
    _pad7: u32,
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
// AO texture + sampler
@group(0) @binding(8) var ao_tex: texture_2d<f32>;
@group(0) @binding(9) var ao_sampler: sampler;
// hm5m close tier
@group(0) @binding(10) var hm5m_tex: texture_2d<f32>;
@group(0) @binding(11) var hm5m_samp: sampler;
@group(0) @binding(12) var<storage, read> hm5m_nx: array<f32>;
@group(0) @binding(13) var<storage, read> hm5m_ny: array<f32>;
@group(0) @binding(14) var<storage, read> hm5m_nz: array<f32>;
@group(0) @binding(15) var<storage, read> hm5m_shadow: array<f32>;

// Width of the blend zone at the edge of any tier boundary, in metres.
// smoothstep from 0 (edge, fully base tier) to BLEND_MARGIN (inner, fully close tier).
const BLEND_MARGIN: f32 = 500.0;

var<private> ssao_16x_kernel: array<vec3<f32>, 16> = array<vec3<f32>, 16>(
    // x = cos(el) * cos(az)
    // y = cos(el) * sin(az)
    // z = sin(el)
    //   Ring │ Elevation │ Azimuth │
    // ├──────┼───────────┼─────────┼
    // │ 1    │ 30°       │ 0°      │
    // ├──────┼───────────┼─────────┼
    // │ 1    │ 30°       │ 90°     │
    // ├──────┼───────────┼─────────┼
    // │ 1    │ 30°       │ 180°    │
    // ├──────┼───────────┼─────────┼
    // │ 1    │ 30°       │ 270°    │
    // ├──────┼───────────┼─────────┼
    // │ 2    │ 60°       │ 45°     │
    // ├──────┼───────────┼─────────┼
    // │ 2    │ 60°       │ 135°    │
    // ├──────┼───────────┼─────────┼
    // │ 2    │ 60°       │ 225°    │
    // ├──────┼───────────┼─────────┼
    // │ 2    │ 60°       │ 315°    │
    // ├──────┼───────────┼─────────┼
    // │ 3    │ 45°       │ 22.5°   │
    // ├──────┼───────────┼─────────┼
    // │ 3    │ 45°       │ 112.5°  │
    // ├──────┼───────────┼─────────┼
    // │ 3    │ 45°       │ 202.5°  │
    // ├──────┼───────────┼─────────┼
    // │ 3    │ 45°       │ 292.5°  │
    // ├──────┼───────────┼─────────┼
    // │ 4    │ 15°       │ 45°     │
    // ├──────┼───────────┼─────────┼
    // │ 4    │ 15°       │ 135°    │
    // ├──────┼───────────┼─────────┼
    // │ 4    │ 15°       │ 225°    │
    // ├──────┼───────────┼─────────┼
    // │ 4    │ 15°       │ 315°    │
    // ├──────┼───────────┼─────────┼
    // ring 1
    vec3<f32>(0.866, 0.0, 0.5),
    vec3<f32>(0.0, 0.866, 0.5),
    vec3<f32>(-0.866, 0.0, 0.5),
    vec3<f32>(0.0, -0.866, 0.5),
    // ring 2
    vec3<f32>(0.354, 0.354, 0.866),
    vec3<f32>(-0.354, 0.354, 0.866),
    vec3<f32>(-0.354, -0.354, 0.866),
    vec3<f32>(0.354, -0.354, 0.866),
    // ring 3
    vec3<f32>(0.653, 0.271, 0.707),
    vec3<f32>(-0.271, 0.653, 0.707),
    vec3<f32>(-0.653, -0.271, 0.707),
    vec3<f32>(0.271, -0.653, 0.707),
    // ring 4
    vec3<f32>(0.683, 0.683, 0.259),
    vec3<f32>(-0.683, 0.683, 0.259),
    vec3<f32>(-0.683, -0.683, 0.259),
    vec3<f32>(0.683, -0.683, 0.259),
);

var<private> hbao_8x_kernel: array<vec2<f32>, 8> = array<vec2<f32>, 8>(
    // │ Angle │   dx   │   dy   │
    // ├───────┼────────┼────────┤
    // │ 0°    │ 1.0    │ 0.0    │
    // ├───────┼────────┼────────┤
    // │ 45°   │ 0.707  │ 0.707  │
    // ├───────┼────────┼────────┤
    // │ 90°   │ 0.0    │ 1.0    │
    // ├───────┼────────┼────────┤
    // │ 135°  │ -0.707 │ 0.707  │
    // ├───────┼────────┼────────┤
    // │ 180°  │ -1.0   │ 0.0    │
    // ├───────┼────────┼────────┤
    // │ 225°  │ -0.707 │ -0.707 │
    // ├───────┼────────┼────────┤
    // │ 270°  │ 0.0    │ -1.0   │
    // ├───────┼────────┼────────┤
    // │ 315°  │ 0.707  │ -0.707 │
    // └───────┴────────┴────────┘
    // x4
    vec2<f32>(1.0, 0.0),
    vec2<f32>(0.707, 0.707),
    vec2<f32>(0.0, 1.0),
    vec2<f32>(-0.707, 0.707),
    // x8
    vec2<f32>(-1.0, 0.0),
    vec2<f32>(-0.707, -0.707),
    vec2<f32>(0.0, -1.0),
    vec2<f32>(0.707, -0.707),
);

// Distance from pos_xy to the nearest edge of the 5m close tier, in metres.
// Returns a negative value (or very small positive) when outside the extent.
// Used as the smoothstep input for tier blending.
fn close_tier_edge_dist(lx: f32, ly: f32) -> f32 {
    return min(min(lx, cam.hm5m_extent_x - lx), min(ly, cam.hm5m_extent_y - ly));
}

// Height sample for the binary-search refinement pass.
// Blends base and close-tier heights in the BLEND_MARGIN zone at the tier boundary.
fn sample_h_exact(pos_xy: vec2<f32>) -> f32 {
    let uv_base = vec2<f32>(
        (pos_xy.x / cam.dx_meters + 0.5) / f32(cam.hm_cols),
        (pos_xy.y / cam.dy_meters + 0.5) / f32(cam.hm_rows),
    );
    let h_base = textureSampleLevel(hm_tex, hm_sampler, uv_base, 0.0).r;
    if cam.hm5m_extent_x > 0.0 {
        let lx = pos_xy.x - cam.hm5m_origin_x;
        let ly = pos_xy.y - cam.hm5m_origin_y;
        if lx >= 0.0 && lx < cam.hm5m_extent_x && ly >= 0.0 && ly < cam.hm5m_extent_y {
            let uv5 = vec2<f32>(lx / cam.hm5m_extent_x, ly / cam.hm5m_extent_y);
            let h5 = textureSampleLevel(hm5m_tex, hm5m_samp, uv5, 0.0).r;
            return mix(h_base, h5, smoothstep(0.0, BLEND_MARGIN, close_tier_edge_dist(lx, ly)));
        }
    }
    return h_base;
}

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

        let h_mid = sample_h_exact(p_mid.xy);
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
        // sky early exit — upward ray already above max terrain height
        // 10k is the limit, since Everest is lower than 10k,
        if dir.z > 0.0 && pos.z > 10000.0 { break; }

        let uv: vec2<f32> = vec2<f32>(
            (pos.x / cam.dx_meters + 0.5) / f32(cam.hm_cols),
            (pos.y / cam.dy_meters + 0.5) / f32(cam.hm_rows)
        );
        // lod_mode: 0=Ultra(off), 1=High, 2=Mid(default), 3=Low
        let lod_step_div = select(select(select(4000.0, 8000.0, cam.lod_mode < 3u), 20000.0, cam.lod_mode < 2u), 1000000000.0, cam.lod_mode == 0u);
        let lod_mip_div = select(select(select(8000.0, 15000.0, cam.lod_mode < 3u), 30000.0, cam.lod_mode < 2u), 1000000000.0, cam.lod_mode == 0u);
        let mip_lod = log2(1.0 + t / lod_mip_div);  // mipmap downsampling start at distance CONFIG_PERFORMANCE

        // ── height sample with blend zone ──
        let lx_loop = pos.x - cam.hm5m_origin_x;
        let ly_loop = pos.y - cam.hm5m_origin_y;
        let in_close_l = cam.hm5m_extent_x > 0.0 && lx_loop >= 0.0 && lx_loop < cam.hm5m_extent_x && ly_loop >= 0.0 && ly_loop < cam.hm5m_extent_y;
        var h: f32;
        if in_close_l {
            let uv5 = vec2<f32>(lx_loop / cam.hm5m_extent_x, ly_loop / cam.hm5m_extent_y);
            let h5 = textureSampleLevel(hm5m_tex, hm5m_samp, uv5, 0.0).r;
            let h_b = textureSampleLevel(hm_tex, hm_sampler, uv, mip_lod).r;
            h = mix(h_b, h5, smoothstep(0.0, BLEND_MARGIN, close_tier_edge_dist(lx_loop, ly_loop)));
        } else {
            h = textureSampleLevel(hm_tex, hm_sampler, uv, mip_lod).r;
        }

        if pos.z <= h {
            hit = true;
            // use t_prev as lower bracket (correct for adaptive steps)
            // pos = binary_search_hit(t - cam.step_m, t, dir, 8);
            pos = binary_search_hit(t_prev, t, dir, 32);
            break;
        }

        // adaptive step: scale by distance above terrain (sphere tracing)
        // safety factor 0.5 — conservative enough for steep mountain slopes
        // cam.step_m is the minimum step to prevent stalling near-surface
        t_prev = t;
        // t += cam.step_m;
        // t += max((pos.z - h) * 0.3, cam.step_m);
        let lod_min_step = cam.step_m * (0.7 + t / lod_step_div);  // step reduction with distance CONFIG_PERFORMANCE
        // vat_mode: 0=Ultra→0.1, 1=High→0.2, 2=Mid→0.4, 3=Low→0.8
        let sphere_factor = select(select(select(0.8, 0.4, cam.vat_mode < 3u), 0.2, cam.vat_mode < 2u), 0.1, cam.vat_mode == 0u);
        t += max((pos.z - h) * sphere_factor, lod_min_step);  // step reduction spherical CONFIG_PERFORMANCE
        pos = cam.origin + dir * t;
    }

    if hit {
        // ── blend factor for the hit point ──
        let lx_hit = pos.x - cam.hm5m_origin_x;
        let ly_hit = pos.y - cam.hm5m_origin_y;
        let in_close_hit = cam.hm5m_extent_x > 0.0 && lx_hit >= 0.0 && lx_hit < cam.hm5m_extent_x && ly_hit >= 0.0 && ly_hit < cam.hm5m_extent_y;
        // t5 = 0: fully base tier, t5 = 1: fully close tier
        let t5 = select(
            0.0,
            smoothstep(0.0, BLEND_MARGIN, close_tier_edge_dist(lx_hit, ly_hit)),
            in_close_hit
        );

        // ── base tier normals and shadow (always computed) ──
        let col_f = pos.x / cam.dx_meters;
        let row_f = pos.y / cam.dy_meters;
        let col0 = i32(col_f);
        let row0 = i32(row_f);
        let fx = col_f - f32(col0);
        let fy = row_f - f32(row0);
        let base_uv = vec2<f32>(col_f / f32(cam.hm_cols), row_f / f32(cam.hm_rows));
        let i00 = u32(row0) * cam.hm_cols + u32(col0);
        let i10 = u32(row0) * cam.hm_cols + u32(col0 + 1);
        let i01 = u32(row0 + 1) * cam.hm_cols + u32(col0);
        let i11 = u32(row0 + 1) * cam.hm_cols + u32(col0 + 1);
        let n_base = normalize(mix(
            mix(vec3<f32>(nx[i00], ny[i00], nz[i00]), vec3<f32>(nx[i10], ny[i10], nz[i10]), fx),
            mix(vec3<f32>(nx[i01], ny[i01], nz[i01]), vec3<f32>(nx[i11], ny[i11], nz[i11]), fx), fy
        ));
        let sh_base = mix(mix(shadow[i00], shadow[i10], fx), mix(shadow[i01], shadow[i11], fx), fy);

        var normal: vec3<f32>;
        var in_shadow: f32;
        var hit_uv: vec2<f32>;

        if t5 > 0.0 {
            // ── close tier normals and shadow ──
            let dx5 = cam.hm5m_extent_x / f32(cam.hm5m_cols);
            let dy5 = cam.hm5m_extent_y / f32(cam.hm5m_rows);
            let c5_f = lx_hit / dx5;
            let r5_f = ly_hit / dy5;
            let c5 = clamp(i32(c5_f), 0, i32(cam.hm5m_cols) - 2);
            let r5 = clamp(i32(r5_f), 0, i32(cam.hm5m_rows) - 2);
            let fx5 = c5_f - f32(c5);
            let fy5 = r5_f - f32(r5);
            let i5_00 = u32(r5) * cam.hm5m_cols + u32(c5);
            let i5_10 = u32(r5) * cam.hm5m_cols + u32(c5 + 1);
            let i5_01 = u32(r5 + 1) * cam.hm5m_cols + u32(c5);
            let i5_11 = u32(r5 + 1) * cam.hm5m_cols + u32(c5 + 1);
            let n5 = normalize(mix(
                mix(vec3<f32>(hm5m_nx[i5_00], hm5m_ny[i5_00], hm5m_nz[i5_00]),
                    vec3<f32>(hm5m_nx[i5_10], hm5m_ny[i5_10], hm5m_nz[i5_10]), fx5),
                mix(vec3<f32>(hm5m_nx[i5_01], hm5m_ny[i5_01], hm5m_nz[i5_01]),
                    vec3<f32>(hm5m_nx[i5_11], hm5m_ny[i5_11], hm5m_nz[i5_11]), fx5), fy5
            ));
            let sh5 = mix(mix(hm5m_shadow[i5_00], hm5m_shadow[i5_10], fx5),
                mix(hm5m_shadow[i5_01], hm5m_shadow[i5_11], fx5), fy5);
            let close_uv = vec2<f32>(lx_hit / cam.hm5m_extent_x, ly_hit / cam.hm5m_extent_y);

            normal = normalize(mix(n_base, n5, t5));
            in_shadow = mix(sh_base, sh5, t5);
            hit_uv = mix(base_uv, close_uv, t5);
        } else {
            normal = n_base;
            in_shadow = sh_base;
            hit_uv = base_uv;
        }

        var ao_factor: f32 = 1.0;
        if cam.ao_mode == 5u {
            ao_factor = textureSampleLevel(ao_tex, ao_sampler, hit_uv, 0.0).r;
            // the higher the last value the less pronounced the effect (less darkening)
            ao_factor = mix(1.0, ao_factor, 0.8);
        } else if cam.ao_mode == 1u || cam.ao_mode == 2u {
            let up_ref = select(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), abs(normal.y) < 0.9);
            let T = normalize(cross(normal, up_ref));
            let B = cross(normal, T);
            let N = normal;
            let n_samples: u32 = select(8u, 16u, cam.ao_mode == 2u);

            var open_factor: f32 = 0.0;
            for (var i = 0u; i < n_samples; i++) {
                let kernel_dir = ssao_16x_kernel[i];
                let world_d = T * kernel_dir.x + B * kernel_dir.y + N * kernel_dir.z;
                let sampling_distance = 600.0;
                let sample_pos = pos + world_d * sampling_distance;

                let sample_uv: vec2<f32> = vec2<f32>(
                    (sample_pos.x / cam.dx_meters + 0.5) / f32(cam.hm_cols),
                    (sample_pos.y / cam.dy_meters + 0.5) / f32(cam.hm_rows)
                );
                let sample_h = textureSampleLevel(hm_tex, hm_sampler, sample_uv, 0.0).r;

                open_factor += smoothstep(-50.0, 50.0, sample_pos.z - sample_h);
            }
            ao_factor = open_factor / f32(n_samples);
        } else if cam.ao_mode == 3u || cam.ao_mode == 4u {
            let n_samples: u32 = select(4u, 8u, cam.ao_mode == 4u);

            var open_factor: f32 = 0.0;
            for (var i = 0u; i < n_samples; i++) {

                var sampling_distance = 600.0;
                var probe_dist = 25.0;
                var max_angle: f32 = 0.0;
                loop {
                    if probe_dist > sampling_distance {
                        break;
                    }

                    let kernel_dir = hbao_8x_kernel[i];
                    let world_d = vec3<f32>(kernel_dir.x, kernel_dir.y, 0.0);
                    let sample_pos = pos + world_d * probe_dist;

                    let sample_uv: vec2<f32> = vec2<f32>(
                        (sample_pos.x / cam.dx_meters + 0.5) / f32(cam.hm_cols),
                        (sample_pos.y / cam.dy_meters + 0.5) / f32(cam.hm_rows)
                    );
                    let sample_h = textureSampleLevel(hm_tex, hm_sampler, sample_uv, 0.0).r;

                    let cur_angle = atan2(sample_h - sample_pos.z, probe_dist);
                    max_angle = select(max_angle, cur_angle, cur_angle > max_angle);

                    probe_dist += 25.0;
                }
                open_factor += 1.0 - sin(max(0.0, max_angle));
            }
            ao_factor = open_factor / f32(n_samples);
        }

        // dot(normal, sun_dir) — sun_dir must also be normalized
        let normalized_sun_dir = normalize(cam.sun_dir);
        let diffuse = max(0.0, dot(normal, normalized_sun_dir));

        let ambient = 0.15;
        let shadow_factor: f32 = select(1.0, 0.5 + 0.5 * in_shadow, cam.shadows_enabled == 1u);
        let brightness = (ambient * ao_factor + (1.0 - ambient) * diffuse) * shadow_factor;

        // set colors for different heights
        let green = vec3<f32>(120.0, 160.0, 80.0);
        let light_green = vec3<f32>(160.0, 175.0, 130.0);
        let rock = vec3<f32>(200.0, 200.0, 195.0);
        let snow = vec3<f32>(240.0, 245.0, 250.0);

        let t1 = smoothstep(1800.0, 2000.0, pos.z);  // green → light_green
        let t2 = smoothstep(2000.0, 2200.0, pos.z);  // light_green → rock
        let t3 = smoothstep(2600.0, 2800.0, pos.z);  // rock → snow

        let base = mix(mix(mix(green, light_green, t1), rock, t2), snow, t3);
        let base_r = base.x;
        let base_g = base.y;
        let base_b = base.z;

        let r = u32(clamp(base_r * brightness, 0.0, 255.0));
        let g = u32(clamp(base_g * brightness, 0.0, 255.0));
        let b = u32(clamp(base_b * brightness, 0.0, 255.0));

        // add fog/haze in the distance
        let sky = vec3<f32>(135.0, 206.0, 235.0);  // same blue as your sky pixels
        let fog_near = 15000.0;   // fog starts at 15km
        let fog_far = 60000.0;   // fully haze at 60km
        let fog_t = select(0.0, smoothstep(fog_near, fog_far, t), cam.fog_enabled == 1u);

        let fr = mix(f32(r), sky.x, fog_t);
        let fg = mix(f32(g), sky.y, fog_t);
        let fb = mix(f32(b), sky.z, fog_t);

        output[gid.y * cam.img_width + gid.x] = u32(fb) | (u32(fg) << 8u) | (u32(fr) << 16u) | (255u << 24u);  // bgra format
    } else {
        // sky blue
        output[gid.y * cam.img_width + gid.x] = 235u | (206u << 8u) | (135u << 16u) | (255u << 24u);  // bgra format
    }
}

