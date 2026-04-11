mod camera;
mod raymarch;
#[cfg(target_arch = "x86_64")]
mod raymarch_avx2;
#[cfg(target_arch = "aarch64")]
mod raymarch_neon;
mod render;
#[cfg(target_arch = "x86_64")]
mod render_avx2;
#[cfg(target_arch = "aarch64")]
mod render_neon;
mod vector_utils;

pub use camera::{Camera, Ray};
pub use raymarch::raymarch;
#[cfg(target_arch = "x86_64")]
pub use raymarch_avx2::{RayPacketAvx2, raymarch_avx2};
#[cfg(target_arch = "aarch64")]
pub use raymarch_neon::{RayPacket, raymarch_neon};
pub use render::{render, render_par};
#[cfg(target_arch = "x86_64")]
pub use render_avx2::{render_avx2, render_avx2_par};
#[cfg(target_arch = "aarch64")]
pub use render_neon::{render_neon, render_neon_par};

pub fn render_vector(
    cam: &Camera,
    hm: &dem_io::Heightmap,
    normals: &terrain::NormalMap,
    shadow_mask: &terrain::ShadowMask,
    sun_dir: [f32; 3],
    width: u32,
    height: u32,
    step_m: f32,
    t_max: f32,
) -> Vec<u8> {
    #[cfg(target_arch = "aarch64")]
    return render_neon(
        cam,
        hm,
        normals,
        shadow_mask,
        sun_dir,
        width,
        height,
        step_m,
        t_max,
    );

    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx2") && width % 8 == 0 {
        return unsafe {
            render_avx2(
                cam,
                hm,
                normals,
                shadow_mask,
                sun_dir,
                width,
                height,
                step_m,
                t_max,
            )
        };
    }

    // Fallback: scalar (any platform without SIMD, or x86_64 without AVX2, or odd width)
    #[cfg(not(target_arch = "aarch64"))]
    {
        #[cfg(target_arch = "x86_64")]
        eprintln!("[SCALAR FALLBACK] render_vector: AVX2 not detected or width ({width}) not divisible by 8");
        #[cfg(not(target_arch = "x86_64"))]
        eprintln!("[SCALAR FALLBACK] render_vector: no SIMD for this architecture");
        return render(
            cam,
            hm,
            normals,
            shadow_mask,
            sun_dir,
            width,
            height,
            step_m,
            t_max,
        );
    }
}

pub fn render_vector_par(
    cam: &Camera,
    hm: &dem_io::Heightmap,
    normals: &terrain::NormalMap,
    shadow_mask: &terrain::ShadowMask,
    sun_dir: [f32; 3],
    width: u32,
    height: u32,
    step_m: f32,
    t_max: f32,
) -> Vec<u8> {
    #[cfg(target_arch = "aarch64")]
    return render_neon_par(
        cam,
        hm,
        normals,
        shadow_mask,
        sun_dir,
        width,
        height,
        step_m,
        t_max,
    );

    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx2") && width % 8 == 0 {
        return unsafe {
            render_avx2_par(
                cam,
                hm,
                normals,
                shadow_mask,
                sun_dir,
                width,
                height,
                step_m,
                t_max,
            )
        };
    }

    // Fallback: scalar parallel
    #[cfg(not(target_arch = "aarch64"))]
    {
        #[cfg(target_arch = "x86_64")]
        eprintln!("[SCALAR FALLBACK] render_vector_par: AVX2 not detected or width ({width}) not divisible by 8");
        #[cfg(not(target_arch = "x86_64"))]
        eprintln!("[SCALAR FALLBACK] render_vector_par: no SIMD for this architecture");
        return render_par(
            cam,
            hm,
            normals,
            shadow_mask,
            sun_dir,
            width,
            height,
            step_m,
            t_max,
        );
    }
}

use crate::vector_utils::normalize;
use dem_io::Heightmap;
use terrain::{NormalMap, ShadowMask};

pub(crate) fn shade(
    hit: Option<[f32; 3]>,
    hm: &Heightmap,
    normals: &NormalMap,
    shadow_mask: &ShadowMask,
    sun_dir: [f32; 3],
) -> [u8; 3] {
    match hit {
        None => [135, 206, 235],
        Some(p) => {
            let col_f = p[0] / hm.dx_meters as f32;
            let row_f = p[1] / hm.dy_meters as f32;
            // clamp to avoid out-of-bounds at heightmap edges
            let col0 = (col_f as usize).min(hm.cols - 2);
            let row0 = (row_f as usize).min(hm.rows - 2);
            let fx = (col_f - col0 as f32).clamp(0.0, 1.0);
            let fy = (row_f - row0 as f32).clamp(0.0, 1.0);

            let i00 = row0 * hm.cols + col0;
            let i10 = row0 * hm.cols + col0 + 1;
            let i01 = (row0 + 1) * hm.cols + col0;
            let i11 = (row0 + 1) * hm.cols + col0 + 1;

            // bilinear normal blend
            let blerp = |a: f32, b: f32, c: f32, d: f32| -> f32 {
                (a * (1.0 - fx) + b * fx) * (1.0 - fy) + (c * (1.0 - fx) + d * fx) * fy
            };
            let nx = blerp(normals.nx[i00], normals.nx[i10], normals.nx[i01], normals.nx[i11]);
            let ny = blerp(normals.ny[i00], normals.ny[i10], normals.ny[i01], normals.ny[i11]);
            let nz = blerp(normals.nz[i00], normals.nz[i10], normals.nz[i01], normals.nz[i11]);
            let len = (nx * nx + ny * ny + nz * nz).sqrt().max(1e-6);
            let (nx, ny, nz) = (nx / len, ny / len, nz / len);

            let sun = normalize(sun_dir);
            let diffuse = f32::max(0.0, nx * sun[0] + ny * sun[1] + nz * sun[2]);

            let ambient = 0.15;
            // bilinear shadow blend
            let in_shadow = blerp(
                shadow_mask.data[i00], shadow_mask.data[i10],
                shadow_mask.data[i01], shadow_mask.data[i11],
            );
            let shadow_factor: f32 = 0.5 + 0.5 * in_shadow;
            let brightness = (ambient + (1.0 - ambient) * diffuse) * shadow_factor;

            // smooth elevation color bands
            let smoothstep = |e0: f32, e1: f32, x: f32| -> f32 {
                let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
                t * t * (3.0 - 2.0 * t)
            };
            let lerp3 = |a: [f32; 3], b: [f32; 3], t: f32| -> [f32; 3] {
                [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
            };
            let green       = [120.0f32, 160.0, 80.0];
            let light_green = [160.0f32, 175.0, 130.0];
            let rock        = [200.0f32, 200.0, 195.0];
            let snow        = [240.0f32, 245.0, 250.0];
            let t1 = smoothstep(1800.0, 2000.0, p[2]);
            let t2 = smoothstep(2000.0, 2200.0, p[2]);
            let t3 = smoothstep(2600.0, 2800.0, p[2]);
            let base = lerp3(lerp3(lerp3(green, light_green, t1), rock, t2), snow, t3);

            let r = (base[0] * brightness).clamp(0.0, 255.0) as u8;
            let g = (base[1] * brightness).clamp(0.0, 255.0) as u8;
            let b = (base[2] * brightness).clamp(0.0, 255.0) as u8;

            [r, g, b]
        }
    }
}

pub(crate) fn elevation_color(hit: Option<[f32; 3]>) -> [u8; 3] {
    match hit {
        None => [135, 206, 235],
        Some(p) => {
            let z: f32 = p[2];
            if z < 1500.0 {
                [34, 139, 34]
            }
            // green
            else if z < 2500.0 {
                [128, 128, 128]
            }
            // grey
            else {
                [255, 255, 255]
            }
            // snow
        }
    }
}
