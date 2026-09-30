@group(1) @binding(0) var dash_texture: texture_2d<f32>;
@group(1) @binding(1) var dash_sampler: sampler;
@group(1) @binding(2) var<uniform> dash_period: vec4<f32>;
@group(1) @binding(3) var ramp_texture: texture_2d<f32>;
@group(1) @binding(4) var ramp_sampler: sampler;

struct FragmentInput {
    @location(0) v_color: vec4<f32>,
    @location(1) v_normal: vec2<f32>,
    @location(2) v_width2: vec2<f32>,
    @location(3) v_gamma_scale: f32,
    @location(4) horizon_distance: f32,
    @location(5) tile_x: f32,
    @location(6) @interpolate(flat) clip_antimeridian: u32,
    @location(7) dash: vec2<f32>,
    @location(8) progress: f32,
    @location(9) across: f32,
    @location(10) blur: f32,
};

struct Output {
    @location(0) out_color: vec4<f32>,
};

@fragment
fn main(in: FragmentInput) -> Output {
    // Dashes stretch with the fractional zoom; a pattern keeps `dash_period.w` for its height.
    let dash_scale = select(1.0, dash_period.w, dash_period.y < 1.5);
    // A dash with round caps has a row of the texture for each step across the line.
    let round_dash = dash_period.z > 0.5 && dash_period.y < 1.5;
    let dash_v = select(0.5, (7.5 - 7.0 * in.across) / 15.0, round_dash);
    let distance_sample = textureSample(dash_texture, dash_sampler,
        vec2<f32>(in.dash.x / max(dash_period.x * dash_scale, 1e-6), dash_v)).r;
    let dash_alpha = select(clamp(0.5 + (distance_sample * 255.0 - 128.0) / 254.0 * dash_period.x * dash_scale * in.dash.y, 0.0, 1.0),
        1.0, dash_period.x <= 0.0);
    if in.horizon_distance < 0.0 {
        discard;
    }
    if in.clip_antimeridian != 0u && (in.tile_x < 0.0 || in.tile_x >= 4096.0) {
        discard;
    }
    // Calculate the distance of the pixel from the line in pixels
    let dist = length(in.v_normal) * in.v_width2.x;

    let pixel_ratio = 1.0; 
    let blur = in.blur;
    
    // Calculate the antialiasing fade factor
    let blur2 = (blur + (1.0 / pixel_ratio)) * in.v_gamma_scale;
    let denom = max(blur2, 1e-6);
    let alpha = clamp(min(dist - (in.v_width2.y - blur2), in.v_width2.x - dist) / denom, 0.0, 1.0);

    // Output non-premultiplied alpha: the blend state (SrcAlpha, OneMinusSrcAlpha)
    // handles the premultiplication. Using v_color * alpha here would double-apply alpha.
    // With a gradient the colour comes from the ramp and the layer's opacity from the alpha.
    let ramp = textureSample(ramp_texture, ramp_sampler, vec2<f32>(clamp(in.progress, 0.0, 1.0), 0.5));
    // A pattern repeats along the line with its height fitted to the line's width.
    if dash_period.y > 1.5 {
        let width_px = max(in.dash.y, 1e-6);
        let along = fract(in.dash.x * width_px / dash_period.z * dash_period.w / width_px);
        let texel = textureSample(ramp_texture, ramp_sampler, vec2<f32>(along, 0.5 * in.across + 0.5));
        let pattern_coverage = texel.a * in.v_color.a * alpha;
        if pattern_coverage < 0.01 { discard; }
        // The pattern is premultiplied, so filtering does not fringe its edges; blending
        // takes straight colours.
        return Output(vec4<f32>(texel.rgb / max(texel.a, 1e-4), pattern_coverage));
    }
    let use_ramp = dash_period.y > 0.5;
    let color = select(in.v_color.rgb, ramp.rgb, use_ramp);
    let opacity = select(in.v_color.a, in.v_color.a * ramp.a, use_ramp);
    let coverage = opacity * alpha * dash_alpha;
    if coverage < 0.01 { discard; }
    return Output(vec4<f32>(color, coverage));
}
