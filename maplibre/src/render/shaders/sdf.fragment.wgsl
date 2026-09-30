// @include symbol_uniforms.wgsl
struct VertexOutput {
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) kind: u32,
    @location(2) size: f32,
    @location(3) opacity: f32,
    @location(4) horizon_distance: f32,
    @location(5) @interpolate(flat) fill: vec4<f32>,
    @location(6) @interpolate(flat) halo: vec4<f32>,
    @location(7) @interpolate(flat) metrics: vec4<f32>,
    @builtin(position) position: vec4<f32>,
};
@group(1) @binding(0) var atlas: texture_2d<f32>;
@group(1) @binding(1) var atlas_sampler: sampler;

fn shade(in: VertexOutput, mode: u32) -> vec4<f32> {
    let sample = textureSample(atlas, atlas_sampler, in.uv);
    let distance = select(sample.a, sample.r, in.kind == 0u);
    // Euclidean coverage keeps diagonal strokes as crisp as horizontal strokes.
    let derivative = max(0.84 * length(vec2<f32>(dpdx(distance), dpdy(distance))), 0.015);
    var color: vec4<f32>;
    if in.kind == 1u {
        if mode == 1u { discard; }
        color = vec4<f32>(sample.rgb * sample.a, sample.a);
    } else {
        let is_text = in.kind == 0u;
        let fill = in.fill;
        let halo = in.halo;
        let metrics = in.metrics;
        let scale = max(select(in.size, in.size / 24.0, is_text), 0.01);
        let fill_coverage = smoothstep(0.75 - derivative, 0.75 + derivative, distance);
        let fill_alpha = fill_coverage * fill.a;
        let width = select(metrics.y, min(metrics.y, in.size * 0.25), is_text);
        let edge = 0.75 - width / (8.0 * scale);
        let softness = derivative + metrics.z / (8.0 * scale);
        let halo_alpha = select(0.0, smoothstep(edge-softness, edge+softness, distance) * halo.a, width > 0.0);
        if mode == 1u {
            let ring = halo_alpha * (1.0 - fill_coverage);
            color = vec4<f32>(halo.rgb * ring, ring);
        } else if mode == 2u {
            color = vec4<f32>(fill.rgb * fill_alpha, fill_alpha);
        } else {
            color = vec4<f32>(fill.rgb * fill_alpha + halo.rgb * halo_alpha * (1.0-fill_alpha),
                fill_alpha + halo_alpha * (1.0-fill_alpha));
        }
    }
    color *= in.opacity;
    // Transparent texels must not overwrite terrain or the compositor's coverage depth.
    if in.horizon_distance < 0.0 || color.a < 0.01 { discard; }
    return color;
}

@fragment
fn main(in: VertexOutput) -> @location(0) vec4<f32> { return shade(in, 0u); }

@fragment
fn halo(in: VertexOutput) -> @location(0) vec4<f32> { return shade(in, 1u); }

@fragment
fn fill(in: VertexOutput) -> @location(0) vec4<f32> { return shade(in, 2u); }
