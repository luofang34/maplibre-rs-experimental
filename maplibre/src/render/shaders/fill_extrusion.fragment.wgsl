struct Output {
    @location(0) out_color: vec4<f32>,
};

// Only the pattern entry point reads these; the plain pipelines have no second bind group.
@group(1) @binding(0) var<uniform> pattern_size: vec4<f32>;
@group(1) @binding(1) var pattern_texture: texture_2d<f32>;
@group(1) @binding(2) var pattern_sampler: sampler;

@fragment
fn main(
    @location(0) v_color: vec4<f32>,
    @location(4) horizon_distance: f32,
) -> Output {
    if horizon_distance < 0.0 {
        discard;
    }
    return Output(v_color);
}

@fragment
fn pattern_main(
    @location(4) horizon_distance: f32,
    @location(5) pattern_pixels: vec2<f32>,
    @location(6) @interpolate(flat) origin_upper: vec2<f32>,
    @location(8) @interpolate(flat) origin_lower: vec2<f32>,
    @location(7) lighting: vec4<f32>,
) -> Output {
    if horizon_distance < 0.0 {
        discard;
    }
    let size = pattern_size.xy;
    let upper = origin_upper - size * floor(origin_upper / size);
    let coarse = upper * 256.0 - size * floor(upper * 256.0 / size);
    let low = coarse * 256.0 + origin_lower;
    let offset = low - size * floor(low / size);
    let uv = fract((pattern_pixels + offset) / size);
    let texel = textureSample(pattern_texture, pattern_sampler, uv);
    // The image is straight alpha; the light and the opacity scale it as a premultiplied colour.
    return Output(vec4<f32>(texel.rgb * texel.a, texel.a) * lighting);
}
