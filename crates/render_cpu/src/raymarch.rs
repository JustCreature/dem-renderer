use crate::vector_utils::*;

use crate::Ray;
use dem_io::Heightmap;

pub fn raymarch(ray: &Ray, hm: &Heightmap, step_m: f32, t_max: f32) -> Option<[f32; 3]> {
    let mut t: f32 = 0.0f32;

    while t < t_max {
        // 1. Current position in worlkd space
        let p = add(ray.origin, scale(ray.dir, t));

        // 2. convert heitmap to pixel indices
        let col: isize = (p[0] / hm.dx_meters as f32) as isize;
        let row: isize = (p[1] / hm.dy_meters as f32) as isize;

        // 3. bounds check - ray left the heightmap, no hit
        if col < 0 || row < 0 || col >= hm.cols as isize || row >= hm.rows as isize {
            return None;
        }

        // 4. sample terrain height at htis pixel
        let terrain_z: f32 = hm.data[row as usize * hm.cols + col as usize] as f32;

        // 5. hit test
        if p[2] < terrain_z {
            let hit = binary_search_hit(ray, hm, t - step_m, t, 8);
            return Some(hit);
        }

        t += step_m;
    }

    None
}

fn binary_search_hit(
    ray: &Ray,
    hm: &Heightmap,
    mut t_lo: f32,
    mut t_hi: f32,
    iterations: u32,
) -> [f32; 3] {
    for _ in 0..iterations {
        let t_mid = (t_lo + t_hi) * 0.5;
        let p = add(ray.origin, scale(ray.dir, t_mid));

        let col = (p[0] / hm.dx_meters as f32) as usize;
        let row = (p[1] / hm.dy_meters as f32) as usize;
        let terrain_z = hm.data[row * hm.cols + col] as f32;

        if p[2] < terrain_z {
            t_hi = t_mid; // still below — move upper bound down
        } else {
            t_lo = t_mid; // above — move lower bound up
        }
    }

    add(ray.origin, scale(ray.dir, t_lo)) // return last above-ground position
}
