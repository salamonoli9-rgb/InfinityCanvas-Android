// Puts one stroke layer (premultiplied alpha) over the screen with the layer's opacity.
@group(0) @binding(0) var layer: texture_2d<f32>;
// Opacity per layer position (x component); the layer position is the instance index.
@group(0) @binding(1) var<uniform> opacity: array<vec4<f32>, 64>;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) layer: u32,
};

@vertex
fn vs(@builtin(vertex_index) i: u32, @builtin(instance_index) l: u32) -> VOut {
    // One triangle covering the whole screen.
    let x = f32((i << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(i & 2u) * 2.0 - 1.0;
    var o: VOut;
    o.pos = vec4<f32>(x, y, 0.0, 1.0);
    o.layer = min(l, 63u);
    return o;
}

@fragment
fn fs(i: VOut) -> @location(0) vec4<f32> {
    return textureLoad(layer, vec2<i32>(i.pos.xy), 0) * opacity[i.layer].x;
}
