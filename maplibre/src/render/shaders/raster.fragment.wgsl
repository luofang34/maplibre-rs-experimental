struct VertexOutput {
    @location(0) tex_coords: vec3<f32>,
    @location(1) horizon_distance: f32,
    @builtin(position) position: vec4<f32>,
};

struct RasterPaint {
    spin_weights: vec4<f32>,
    opacity: f32,
    saturation_factor: f32,
    contrast_factor: f32,
    brightness_min: f32,
    brightness_max: f32,
    nearest: f32,
    align: vec2<f32>,
};

@group(1) @binding(0)
var t_diffuse: texture_2d<f32>;
@group(1) @binding(1)
var s_diffuse: sampler;
@group(2) @binding(0)
var<uniform> paint: RasterPaint;

@fragment
fn main(in: VertexOutput) -> @location(0) vec4<f32> {
    if in.horizon_distance < 0.0 {
        discard;
    }
    var uv = in.tex_coords.xy / in.tex_coords.z;
    if paint.nearest > 0.5 {
        let size = vec2<f32>(textureDimensions(t_diffuse));
        uv = (floor(uv * size) + 0.5) / size;
    }
    let color = textureSample(t_diffuse, s_diffuse, uv);
    let alpha = color.a * paint.opacity;
    var rgb = color.rgb;
    let w = paint.spin_weights.xyz;
    rgb = vec3<f32>(dot(rgb, w), dot(rgb, w.zxy), dot(rgb, w.yzx));
    let average = (color.r + color.g + color.b) / 3.0;
    rgb += (average - rgb) * paint.saturation_factor;
    rgb = (rgb - 0.5) * paint.contrast_factor + 0.5;
    rgb = mix(vec3<f32>(paint.brightness_min), vec3<f32>(paint.brightness_max), rgb);
    return vec4<f32>(rgb, alpha);
}
