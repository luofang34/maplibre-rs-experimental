struct PatternUniforms {
    size: vec4<f32>,
};

@group(1) @binding(0) var<uniform> pattern: PatternUniforms;
@group(1) @binding(1) var pattern_texture: texture_2d<f32>;
@group(1) @binding(2) var pattern_sampler: sampler;

@fragment
fn main(
    @location(0) v_color: vec4<f32>,
    @location(1) pattern_position: vec2<f32>,
    @location(4) horizon_distance: f32,
) -> @location(0) vec4<f32> {
    if horizon_distance < 0.0 {
        discard;
    }
    let uv = fract(pattern_position / pattern.size.xy);
    let texel = textureSample(pattern_texture, pattern_sampler, uv);
    // The texture holds premultiplied colours, so filtering does not fringe the edges.
    return texel * v_color.a;
}
