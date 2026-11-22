struct Uniforms {
    viewport: vec2<f32>,
    _pad: vec2<f32>,
};
@group(0) @binding(0) var<uniform> u: Uniforms;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs(
    @location(0) p: vec2<f32>,
    @location(1) offset: vec2<f32>,
    @location(2) scale: f32,
    @location(3) color: vec4<f32>,
) -> VOut {
    let s = offset + p * scale;
    var o: VOut;
    o.pos = vec4<f32>(s.x / u.viewport.x * 2.0 - 1.0, 1.0 - s.y / u.viewport.y * 2.0, 0.0, 1.0);
    o.color = color;
    return o;
}

@fragment
fn fs(i: VOut) -> @location(0) vec4<f32> {
    return i.color;
}
