mod camera;
mod raymarch;
mod raymarch_neon;
mod render;
mod render_neon;
mod vector_utils;

pub use camera::{Camera, Ray};
pub use raymarch::raymarch;
pub use raymarch_neon::{RayPacket, raymarch_neon};
pub use render::{render, render_par};
pub use render_neon::{render_neon, render_neon_par};

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
            let col = (p[0] / hm.dx_meters as f32) as usize;
            let row = (p[1] / hm.dy_meters as f32) as usize;
            let idx = row * hm.cols + col;

            let nx = normals.nx[idx];
            let ny = normals.ny[idx];
            let nz = normals.nz[idx];

            // let sun = normalize([0.5, 0.5, 0.7]);
            // let sun = normalize([0.4, 0.5, 0.7]); // more east, less south
            let sun = normalize(sun_dir);
            // dot(normal, sun_dir) — sun_dir must also be normalized
            let diffuse = f32::max(0.0, nx * sun[0] + ny * sun[1] + nz * sun[2]);

            let ambient = 0.15;
            // let ambient = 0.3; // less dark shadow faces
            let in_shadow: f32 = shadow_mask.data[idx];
            let shadow_factor: f32 = 0.5 + 0.5 * in_shadow;
            let brightness = (ambient + (1.0 - ambient) * diffuse) * shadow_factor;

            // let base = [180u8, 160u8, 140u8]; // rocky grey-brown
            // let base = [200u8, 200u8, 195u8]; // cool grey-white
            let base = if p[2] < 1900.0 {
                [120u8, 160u8, 80u8] // green                                                                                                                            
            } else if p[2] < 2100.0 {
                [160u8, 175u8, 130u8] // slightly green                                                                                                                   
            } else if p[2] < 2700.0 {
                [200u8, 200u8, 195u8] // grey-white rock                                                                                                                
            } else {
                [240u8, 245u8, 250u8] // glacier snow white                                                                                                             
            };
            let r = (base[0] as f32 * brightness) as u8;
            let g = (base[1] as f32 * brightness) as u8;
            let b = (base[2] as f32 * brightness) as u8;

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
