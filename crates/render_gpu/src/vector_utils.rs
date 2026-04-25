pub(crate) fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub(crate) fn scale(a: [f32; 3], b: f32) -> [f32; 3] {
    [a[0] * b, a[1] * b, a[2] * b]
}

pub(crate) fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub(crate) fn normalize(a: [f32; 3]) -> [f32; 3] {
    let length: f32 = (a[0].powi(2) + a[1].powi(2) + a[2].powi(2)).sqrt();

    [a[0] / length, a[1] / length, a[2] / length]
}

pub(crate) fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    let x: f32 = a[1] * b[2] - a[2] * b[1];
    let y: f32 = a[2] * b[0] - a[0] * b[2];
    let z: f32 = a[0] * b[1] - a[1] * b[0];

    [x, y, z]
}
