//! Fly-through camera, extracted from the inline camera state + control logic in
//! the binary's `viewer/mod.rs`. Pure input → state; bounds-clamping against the
//! loaded heightmap stays in `ViewerCore` (it needs the scene extent).

use std::collections::HashSet;

use winit::keyboard::KeyCode;

/// Horizontal/vertical fly speed at normal and boosted (Shift) rates.
const BASE_SPEED_M_PER_S: f32 = 500.0;
const BOOST_MULTIPLIER: f32 = 10.0;
const MOUSE_SENSITIVITY: f32 = 0.001;
const PITCH_LIMIT: f32 = 1.57;

pub struct FlyCamera {
    /// Tile-local metres from the heightmap top-left: [x (east), y (south), z (up)].
    pub pos: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub keys_held: HashSet<KeyCode>,
    /// Left-button drag look (non-immersive). Inverts the mouse delta.
    pub mouse_look: bool,
    /// Q-toggled cursor-locked free-look.
    pub immersive_mode: bool,
    /// Shift-held movement/time boost.
    pub speed_boost: bool,
}

impl FlyCamera {
    pub fn new(pos: [f32; 3], yaw: f32, pitch: f32) -> Self {
        FlyCamera {
            pos,
            yaw,
            pitch,
            keys_held: HashSet::new(),
            mouse_look: false,
            immersive_mode: false,
            speed_boost: false,
        }
    }

    /// Movement speed in metres/second for the current boost state.
    pub fn speed(&self) -> f32 {
        BASE_SPEED_M_PER_S * if self.speed_boost { BOOST_MULTIPLIER } else { 1.0 }
    }

    /// Apply WASD horizontal movement (from yaw only) and Space/Alt vertical
    /// movement for `dt` seconds. Does **not** clamp to the heightmap — the
    /// caller does that since it owns the scene extent.
    pub fn update(&mut self, dt: f32) {
        let speed = self.speed();

        // horizontal movement vectors from yaw only
        let forward_h = [self.yaw.sin(), -self.yaw.cos(), 0.0_f32];
        let right_h = [self.yaw.cos(), self.yaw.sin(), 0.0_f32];

        if self.keys_held.contains(&KeyCode::KeyW) {
            self.pos[0] += forward_h[0] * speed * dt;
            self.pos[1] += forward_h[1] * speed * dt;
        }
        if self.keys_held.contains(&KeyCode::KeyS) {
            self.pos[0] -= forward_h[0] * speed * dt;
            self.pos[1] -= forward_h[1] * speed * dt;
        }
        if self.keys_held.contains(&KeyCode::KeyA) {
            self.pos[0] -= right_h[0] * speed * dt;
            self.pos[1] -= right_h[1] * speed * dt;
        }
        if self.keys_held.contains(&KeyCode::KeyD) {
            self.pos[0] += right_h[0] * speed * dt;
            self.pos[1] += right_h[1] * speed * dt;
        }
        if self.keys_held.contains(&KeyCode::Space) {
            self.pos[2] += speed * dt;
        }
        if self.keys_held.contains(&KeyCode::AltLeft) || self.keys_held.contains(&KeyCode::SuperLeft)
        {
            self.pos[2] -= speed * dt;
        }
    }

    /// Full forward vector including pitch (for `look_at`).
    pub fn forward(&self) -> [f32; 3] {
        [
            self.pitch.cos() * self.yaw.sin(),
            -self.pitch.cos() * self.yaw.cos(),
            self.pitch.sin(),
        ]
    }

    /// Look-at target one unit ahead of the camera.
    pub fn look_at(&self) -> [f32; 3] {
        let fwd = self.forward();
        [
            self.pos[0] + fwd[0],
            self.pos[1] + fwd[1],
            self.pos[2] + fwd[2],
        ]
    }

    /// Apply a raw mouse-motion delta to yaw/pitch. No-op unless a look mode is
    /// active. Non-immersive (left-drag) look inverts the delta, matching the
    /// original behaviour.
    pub fn apply_mouse_delta(&mut self, dx: f64, dy: f64) {
        if !self.mouse_look && !self.immersive_mode {
            return;
        }
        let inversion: f32 = if self.immersive_mode { 1.0 } else { -1.0 };
        self.yaw += dx as f32 * MOUSE_SENSITIVITY * inversion;
        self.pitch -= dy as f32 * MOUSE_SENSITIVITY * inversion;
        self.pitch = self.pitch.clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_north_at_zero_yaw_pitch() {
        // yaw 0, pitch 0: forward = [sin0, -cos0, sin0] = [0, -1, 0] (due "north"/-y).
        let cam = FlyCamera::new([0.0, 0.0, 0.0], 0.0, 0.0);
        let f = cam.forward();
        assert!(f[0].abs() < 1e-6 && (f[1] + 1.0).abs() < 1e-6 && f[2].abs() < 1e-6);
    }

    #[test]
    fn w_moves_along_forward_horizontal() {
        let mut cam = FlyCamera::new([100.0, 100.0, 10.0], 0.0, 0.5);
        cam.keys_held.insert(KeyCode::KeyW);
        cam.update(1.0);
        // At yaw 0, forward_h = [0, -1, 0]; one second at 500 m/s moves -500 on y.
        assert!((cam.pos[1] - (100.0 - 500.0)).abs() < 1e-3);
        assert!((cam.pos[0] - 100.0).abs() < 1e-3, "no x drift at yaw 0");
        assert!((cam.pos[2] - 10.0).abs() < 1e-3, "pitch must not affect WASD");
    }

    #[test]
    fn boost_multiplies_speed() {
        let mut cam = FlyCamera::new([0.0, 0.0, 0.0], 0.0, 0.0);
        cam.speed_boost = true;
        assert!((cam.speed() - 5000.0).abs() < 1e-3);
    }

    #[test]
    fn pitch_is_clamped() {
        let mut cam = FlyCamera::new([0.0, 0.0, 0.0], 0.0, 0.0);
        cam.immersive_mode = true;
        // Large downward delta would drive pitch past the limit.
        cam.apply_mouse_delta(0.0, 1_000_000.0);
        assert!(cam.pitch >= -PITCH_LIMIT - 1e-6 && cam.pitch <= PITCH_LIMIT + 1e-6);
    }

    #[test]
    fn mouse_delta_ignored_without_look_mode() {
        let mut cam = FlyCamera::new([0.0, 0.0, 0.0], 0.3, 0.2);
        cam.apply_mouse_delta(500.0, 500.0);
        assert_eq!((cam.yaw, cam.pitch), (0.3, 0.2));
    }
}
