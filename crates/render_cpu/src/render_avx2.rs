#![cfg(target_arch = "x86_64")]

use dem_io::Heightmap;
use rayon::prelude::*;
use terrain::{NormalMap, ShadowMask};

use crate::raymarch_avx2::{RayPacketAvx2, raymarch_avx2};
use crate::{Camera, Ray, shade};

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub unsafe fn render_avx2(
    cam: &Camera,
    hm: &Heightmap,
    normals: &NormalMap,
    shadow_mask: &ShadowMask,
    sun_dir: [f32; 3],
    width: u32,
    height: u32,
    step_m: f32,
    t_max: f32,
) -> Vec<u8> {
    assert!(
        width % 8 == 0,
        "width must be divisible by 8 for AVX2 packet rendering"
    );
    let mut framebuffer = vec![0u8; (width * height * 3) as usize];

    for py in 0..height {
        for px in (0..width).step_by(8) {
            let r0: Ray = cam.ray_for_pixel(px + 0, py, width, height);
            let r1: Ray = cam.ray_for_pixel(px + 1, py, width, height);
            let r2: Ray = cam.ray_for_pixel(px + 2, py, width, height);
            let r3: Ray = cam.ray_for_pixel(px + 3, py, width, height);
            let r4: Ray = cam.ray_for_pixel(px + 4, py, width, height);
            let r5: Ray = cam.ray_for_pixel(px + 5, py, width, height);
            let r6: Ray = cam.ray_for_pixel(px + 6, py, width, height);
            let r7: Ray = cam.ray_for_pixel(px + 7, py, width, height);

            let packet = RayPacketAvx2::new(&r0, &r1, &r2, &r3, &r4, &r5, &r6, &r7);
            let hits: [Option<[f32; 3]>; 8] = raymarch_avx2(&packet, hm, step_m, t_max);

            for (i, hit) in hits.iter().enumerate() {
                let color: [u8; 3] = shade(*hit, hm, normals, shadow_mask, sun_dir);
                let idx: usize = ((py * width + px + i as u32) * 3) as usize;
                framebuffer[idx] = color[0];
                framebuffer[idx + 1] = color[1];
                framebuffer[idx + 2] = color[2];
            }
        }
    }

    framebuffer
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub unsafe fn render_avx2_par(
    cam: &Camera,
    hm: &Heightmap,
    normals: &NormalMap,
    shadow_mask: &ShadowMask,
    sun_dir: [f32; 3],
    width: u32,
    height: u32,
    step_m: f32,
    t_max: f32,
) -> Vec<u8> {
    assert!(
        width % 8 == 0,
        "width must be divisible by 8 for AVX2 packet rendering"
    );
    let mut framebuffer = vec![0u8; (width * height * 3) as usize];

    framebuffer
        .par_chunks_mut((width * 3) as usize)
        .enumerate()
        .for_each(|(py, row_buf)| {
            for px in (0..width).step_by(8) {
                let r0: Ray = cam.ray_for_pixel(px + 0, py as u32, width, height);
                let r1: Ray = cam.ray_for_pixel(px + 1, py as u32, width, height);
                let r2: Ray = cam.ray_for_pixel(px + 2, py as u32, width, height);
                let r3: Ray = cam.ray_for_pixel(px + 3, py as u32, width, height);
                let r4: Ray = cam.ray_for_pixel(px + 4, py as u32, width, height);
                let r5: Ray = cam.ray_for_pixel(px + 5, py as u32, width, height);
                let r6: Ray = cam.ray_for_pixel(px + 6, py as u32, width, height);
                let r7: Ray = cam.ray_for_pixel(px + 7, py as u32, width, height);

                let packet = RayPacketAvx2::new(&r0, &r1, &r2, &r3, &r4, &r5, &r6, &r7);
                let hits: [Option<[f32; 3]>; 8] = raymarch_avx2(&packet, hm, step_m, t_max);

                for (i, hit) in hits.iter().enumerate() {
                    let color: [u8; 3] = shade(*hit, hm, normals, shadow_mask, sun_dir);
                    let idx: usize = ((px + i as u32) * 3) as usize;
                    row_buf[idx] = color[0];
                    row_buf[idx + 1] = color[1];
                    row_buf[idx + 2] = color[2];
                }
            }
        });

    framebuffer
}
