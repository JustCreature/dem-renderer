use crate::vector_utils::*;

pub struct Camera {
    pub origin: [f32; 3],
    pub forward: [f32; 3],
    pub right: [f32; 3],
    pub up: [f32; 3],
    pub half_w: f32,
    pub half_h: f32,
}

pub struct Ray {
    pub origin: [f32; 3],
    pub dir: [f32; 3],
}

impl Camera {
    pub fn new(origin: [f32; 3], look_at: [f32; 3], fov_deg: f32, aspect: f32) -> Camera {
        let forward: [f32; 3] = normalize(sub(look_at, origin));
        let right: [f32; 3] = normalize(cross(forward, [0.0, 0.0, 1.0]));
        // let right = normalize(cross([0.0, 0.0, 1.0], forward)); // reversed cross
        let up: [f32; 3] = cross(right, forward);
        let half_w: f32 = (fov_deg / 2.0).to_radians().tan();
        let half_h: f32 = half_w / aspect;

        Camera {
            origin,
            forward,
            right,
            up,
            half_w,
            half_h,
        }
    }

    pub fn ray_for_pixel(&self, px: u32, py: u32, w: u32, h: u32) -> Ray {
        let u: f32 = (px as f32 + 0.5) / w as f32;
        let v: f32 = (py as f32 + 0.5) / h as f32;

        // let ndc_x: f32 = 2.0 * u - 1.0;
        let ndc_x: f32 = -(2.0 * u - 1.0); // flip horizontal 
        let ndc_y: f32 = 1.0 - 2.0 * v;

        let cam_x: f32 = ndc_x * self.half_w;
        let cam_y: f32 = ndc_y * self.half_h;

        let dir: [f32; 3] = normalize(add(
            add(self.forward, scale(self.right, cam_x)),
            scale(self.up, cam_y),
        ));

        Ray {
            origin: self.origin,
            dir,
        }
    }
}
