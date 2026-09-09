// @include symbol_uniforms.wgsl
struct VertexOutput {
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) kind: u32,
    @location(2) size: f32,
    @location(3) opacity: f32,
    @location(4) horizon_distance: f32,
    @builtin(position) position: vec4<f32>,
};
@group(1) @binding(0) var atlas: texture_2d<f32>;
@group(1) @binding(1) var atlas_sampler: sampler;

@fragment
fn main(in: VertexOutput) -> @location(0) vec4<f32> {
    let sample = textureSample(atlas, atlas_sampler, in.uv);
    let distance = select(sample.a, sample.r, in.kind == 0u);
    // Euclidean coverage keeps diagonal strokes as crisp as horizontal strokes.
    let derivative = max(0.84 * length(vec2<f32>(dpdx(distance), dpdy(distance))), 0.015);
    var color: vec4<f32>;
    if in.kind == 1u {
        color = vec4<f32>(sample.rgb * sample.a, sample.a);
    } else {
        let is_text = in.kind == 0u;
        let fill = select(symbol.icon_color, symbol.text_color, is_text);
        let halo = select(symbol.icon_halo_color, symbol.halo_color, is_text);
        let metrics = select(symbol.icon, symbol.text, is_text);
        let scale = max(select(in.size, in.size / 24.0, is_text), 0.01);
        let fill_alpha = smoothstep(0.75 - derivative, 0.75 + derivative, distance) * fill.a;
        let width = select(metrics.y, min(metrics.y, in.size * 0.25), is_text);
        let edge = 0.75 - width / (8.0 * scale);
        let softness = derivative + metrics.z / (8.0 * scale);
        let halo_alpha = select(0.0, smoothstep(edge-softness, edge+softness, distance) * halo.a, width > 0.0);
        color = vec4<f32>(fill.rgb * fill_alpha + halo.rgb * halo_alpha * (1.0-fill_alpha),
            fill_alpha + halo_alpha * (1.0-fill_alpha));
    }
    color *= in.opacity;
    // Transparent texels must not overwrite terrain or the compositor's coverage depth.
    if in.horizon_distance < 0.0 || color.a < 0.01 { discard; }
    return color;
}
