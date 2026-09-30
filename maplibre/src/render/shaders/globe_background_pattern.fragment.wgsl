struct PatternUniforms {
    // Width and height of the pattern image in map pixels.
    size: vec4<f32>,
};

// The size of the world in map pixels at the current zoom.
struct WorldUniforms {
    pixels: vec4<f32>,
};

@group(1) @binding(0) var<uniform> pattern: PatternUniforms;
@group(1) @binding(1) var pattern_texture: texture_2d<f32>;
@group(1) @binding(2) var pattern_sampler: sampler;
@group(2) @binding(0) var<uniform> world: WorldUniforms;

struct Output {
    @location(0) out_color: vec4<f32>,
};

@fragment
fn main(
    @location(0) color: vec4<f32>,
    @location(1) mercator: vec2<f32>,
    @location(4) horizon_distance: f32,
) -> Output {
    if horizon_distance < 0.0 {
        discard;
    }
    let uv = fract(mercator * world.pixels.x / pattern.size.xy);
    let texel = textureSample(pattern_texture, pattern_sampler, uv);
    // The texture holds premultiplied colours, so filtering does not fringe the edges.
    return Output(texel * color.a);
}
