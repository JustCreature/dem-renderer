struct ScreenSize {
    width: f32,
    height: f32,
}

@group(0) @binding(0) var<uniform> screen: ScreenSize;

@vertex
fn vs_main(@location(0) pos: vec2<f32>) -> @builtin(position) vec4<f32> {
    let ndc_x = (pos.x / screen.width) * 2.0 - 1.0;
    let ndc_y = 1.0 - (pos.y / screen.height) * 2.0;  // Y is flipped: pixel 0 is top, NDC +1 is top

    return vec4<f32>(ndc_x, ndc_y, 0.0, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 0.0, 0.0, 0.6);
}
