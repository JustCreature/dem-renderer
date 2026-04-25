use dem_io::Heightmap;
use rayon::prelude::*;
use terrain::{NormalMap, ShadowMask};

use crate::shade;
use crate::{Camera, Ray, raymarch};

pub fn render(
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
    let mut framebuffer = vec![0u8; (width * height * 3) as usize];

    for py in 0..height {
        for px in 0..width {
            let ray: Ray = cam.ray_for_pixel(px, py, width, height);
            let hit: Option<[f32; 3]> = raymarch(&ray, hm, step_m, t_max);
            // let color: [u8; 3] = elevation_color(hit);
            let color: [u8; 3] = shade(hit, hm, normals, shadow_mask, sun_dir);
            let idx: usize = ((py * width + px) * 3) as usize;
            framebuffer[idx] = color[0];
            framebuffer[idx + 1] = color[1];
            framebuffer[idx + 2] = color[2];
        }
    }

    framebuffer
}

pub fn render_par(
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
    let mut framebuffer = vec![0u8; (width * height * 3) as usize];

    framebuffer
        .par_chunks_mut((width * 3) as usize)
        .enumerate()
        .for_each(|(py, row_buf)| {
            for px in 0..width {
                let ray: Ray = cam.ray_for_pixel(px, py as u32, width, height);
                let hit: Option<[f32; 3]> = raymarch(&ray, hm, step_m, t_max);
                // let color: [u8; 3] = elevation_color(hit);
                let color: [u8; 3] = shade(hit, hm, normals, shadow_mask, sun_dir);
                let idx: usize = (px * 3) as usize;
                row_buf[idx] = color[0];
                row_buf[idx + 1] = color[1];
                row_buf[idx + 2] = color[2];
            }
        });

    framebuffer
}
