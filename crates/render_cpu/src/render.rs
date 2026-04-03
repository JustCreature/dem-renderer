use dem_io::Heightmap;
use terrain::NormalMap;

use crate::{Camera, Ray, raymarch, vector_utils::normalize};

pub fn render(
    cam: &Camera,
    hm: &Heightmap,
    normals: &NormalMap,
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
            let color: [u8; 3] = shade(hit, hm, normals);
            let idx: usize = ((py * width + px) * 3) as usize;
            framebuffer[idx] = color[0];
            framebuffer[idx + 1] = color[1];
            framebuffer[idx + 2] = color[2];
        }
    }

    framebuffer
}

fn shade(hit: Option<[f32; 3]>, hm: &Heightmap, normals: &NormalMap) -> [u8; 3] {
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
            let sun = normalize([0.4, 0.5, 0.7]); // more east, less south
            // dot(normal, sun_dir) — sun_dir must also be normalized
            let diffuse = f32::max(0.0, nx * sun[0] + ny * sun[1] + nz * sun[2]);

            let ambient = 0.15;
            // let ambient = 0.3; // less dark shadow faces
            let brightness = ambient + (1.0 - ambient) * diffuse;

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

fn elevation_color(hit: Option<[f32; 3]>) -> [u8; 3] {
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
