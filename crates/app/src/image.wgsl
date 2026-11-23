// Reference photos, drawn in screen space below the stroke layer.
struct Uniforms {
    viewport: vec2<f32>,
    _pad: vec2<f32>,
};
@group(0) @binding(0) var<uniform> u: Uniforms;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) opacity: f32,
};

@vertex
fn vs(@location(0) p: vec2<f32>, @location(1) uv: vec2<f32>, @location(2) opacity: f32) -> VOut {
    var o: VOut;
    o.pos = vec4<f32>(p.x / u.viewport.x * 2.0 - 1.0, 1.0 - p.y / u.viewport.y * 2.0, 0.0, 1.0);
    o.uv = uv;
    o.opacity = opacity;
    return o;
}

@fragment
fn fs(i: VOut) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, i.uv);
    let a = c.a * i.opacity;
    return vec4<f32>(c.rgb * a, a);
}
