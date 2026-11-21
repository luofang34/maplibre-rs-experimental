@group(0) @binding(0)
var t_density: texture_2d<f32>;
@group(0) @binding(1)
var s_linear: sampler;

@group(1) @binding(0)
var t_ramp: texture_2d<f32>;

struct HeatmapUniforms {
    opacity: f32,
    padding0: f32,
    padding1: f32,
    padding2: f32,
};

@group(1) @binding(1)
var<uniform> heatmap: HeatmapUniforms;

@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let density = textureSample(t_density, s_linear, uv).r;
    // Densities beyond the ramp saturate at its last colour.
    let color = textureSample(t_ramp, s_linear, vec2<f32>(clamp(density, 0.0, 1.0), 0.5));
    return color * heatmap.opacity;
}
