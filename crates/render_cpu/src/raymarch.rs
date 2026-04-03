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
            return Some(p);
        }

        t += step_m;
    }

    None
}
