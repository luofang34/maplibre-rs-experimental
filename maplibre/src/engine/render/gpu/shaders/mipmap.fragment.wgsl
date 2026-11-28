@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;

// Sampled with a linear filter at the center of each texel of the level below, which
// averages the four texels of the level above.
@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    return textureSample(source, source_sampler, vec2<f32>(uv.x, 1.0 - uv.y));
}
