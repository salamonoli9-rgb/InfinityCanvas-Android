struct Uniforms {
    viewport: vec2<f32>,
    _pad: vec2<f32>,
};
@group(0) @binding(0) var<uniform> u: Uniforms;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    /// Stroke-local position (for the paper grain).
    @location(1) local: vec2<f32>,
    @location(2) intensity: f32,
    /// Grain strength (already faded by magnification), 1 / cell size.
    @location(3) grain: vec2<f32>,
};

@vertex
fn vs(
    @location(0) p: vec3<f32>,
    @location(1) offset: vec2<f32>,
    @location(2) rs: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) params: vec4<f32>,
) -> VOut {
    // offset + R·p·scale, with rs = scale·(cos, sin).
    let s = offset + vec2<f32>(rs.x * p.x - rs.y * p.y, rs.y * p.x + rs.x * p.y);
    var o: VOut;
    o.pos = vec4<f32>(s.x / u.viewport.x * 2.0 - 1.0, 1.0 - s.y / u.viewport.y * 2.0, 0.0, 1.0);
    o.color = color;
    o.local = p.xy;
    o.intensity = p.z;
    // Grain is sized like paper fibres when drawn; fade it out once it is magnified >~16×.
    let scale = max(length(rs), 1e-6);
    let fade = clamp(1.5 - 0.25 * log2(scale), 0.0, 1.0);
    o.grain = vec2<f32>(params.x * fade, params.y);
    return o;
}

fn hash(p: vec2<f32>) -> f32 {
    var q = fract(p * vec2<f32>(0.1031, 0.1030));
    q = q + dot(q, q.yx + 33.33);
    return fract((q.x + q.y) * q.x);
}

fn noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let w = f * f * (3.0 - 2.0 * f);
    let a = mix(hash(i), hash(i + vec2<f32>(1.0, 0.0)), w.x);
    let b = mix(hash(i + vec2<f32>(0.0, 1.0)), hash(i + vec2<f32>(1.0, 1.0)), w.x);
    return mix(a, b, w.y);
}

@fragment
fn fs(i: VOut) -> @location(0) vec4<f32> {
    var k = i.intensity;
    if (i.grain.x > 0.0) {
        let q = i.local * i.grain.y;
        let g = 0.65 * noise(q) + 0.35 * noise(q * 2.7 + vec2<f32>(17.0, 3.0));
        k = k * mix(1.0, smoothstep(0.22, 0.78, g), i.grain.x);
    }
    // Premultiplied colour: scale everything.
    return i.color * k;
}
