struct FragmentInput {
    @location(0) @interpolate(flat) v_color: vec4<f32>,
    @location(1) v_normal: vec2<f32>,
    @location(2) @interpolate(flat) v_width2: vec2<f32>,
    @location(3) @interpolate(flat) blur: f32,
    @location(4) tile_x: f32,
    @location(5) @interpolate(flat) clip_antimeridian: u32,
};

struct Output {
    @location(0) out_color: vec4<f32>,
};

@fragment
fn main(in: FragmentInput) -> Output {
    let nominal_distance = length(in.v_normal) * in.v_width2.x;
    let gamma_scale = clamp(length(vec2<f32>(dpdx(nominal_distance), dpdy(nominal_distance))), 0.25, 64.0);
    let in_antimeridian = in.clip_antimeridian == 0u || (in.tile_x >= 0.0 && in.tile_x < 4096.0);
    let blur2 = (in.blur + 1.0) * gamma_scale;
    let denom = max(blur2, 1e-6);
    let edge_dist = in.v_width2.x - nominal_distance;
    let dist = select(
        edge_dist,
        min(nominal_distance - (in.v_width2.y - blur2), edge_dist),
        in.v_width2.y > 0.0,
    );
    let alpha = clamp(dist / denom, 0.0, 1.0);
    let coverage = in.v_color.a * alpha;
    if !in_antimeridian || coverage < 0.01 {
        discard;
    }
    return Output(vec4<f32>(in.v_color.rgb, coverage));
}
