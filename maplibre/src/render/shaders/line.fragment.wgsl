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
    @location(11) @interpolate(flat) floor_width: f32,
};

struct Output {
    @location(0) out_color: vec4<f32>,
};

@fragment
fn main(in: FragmentInput) -> Output {
    // The distance from the line changes by this many nominal pixels per screen pixel, which
    // is how much the map-space line has narrowed or widened; the edge feather scales with it.
    let nominal_distance = length(in.v_normal) * in.v_width2.x;
    let gamma_scale = select(
        clamp(length(vec2<f32>(dpdx(nominal_distance), dpdy(nominal_distance))), 0.25, 64.0),
        in.v_gamma_scale,
        in.v_gamma_scale > 0.0,
    );
    // Dashes stretch with the fractional zoom; a pattern keeps `dash_period.w` for its height.
    let dash_scale = select(1.0, dash_period.w, dash_period.y < 1.5);
    // A dash with round caps has a row of the texture for each step across the line.
    let round_dash = dash_period.z > 0.5 && dash_period.y < 1.5;
    let dash_v = select(0.5, (7.5 - 7.0 * in.across) / 15.0, round_dash);
    let distance_sample = textureSample(dash_texture, dash_sampler,
        vec2<f32>(in.dash.x / max(dash_period.x * dash_scale, 1e-6), dash_v)).r;
    // A gradient line's dash edge is a smoothstep over a thirtysecond of the pattern's width in
    // alpha units, as GL JS draws it; other dashes ramp over a pixel.
    let edge = 1.0 / max(dash_period.x * in.floor_width, 1e-6);
    let gradient_alpha = smoothstep(0.5 - edge, 0.5 + edge, distance_sample);
    let dash_alpha = select(
        select(
            clamp(0.5 + (distance_sample * 255.0 - 128.0) / 254.0 * dash_period.x * dash_scale * in.dash.y, 0.0, 1.0),
            gradient_alpha,
            dash_period.y > 0.5 && dash_period.y < 1.5,
        ),
        1.0, dash_period.x <= 0.0);
    if in.horizon_distance < 0.0 {
        discard;
    }
    if in.clip_antimeridian != 0u && (in.tile_x < 0.0 || in.tile_x >= 4096.0) {
        discard;
    }
    // Calculate the distance of the pixel from the line in pixels
    let dist = nominal_distance;

    let pixel_ratio = 1.0; 
    let blur = in.blur;
    
    // Calculate the antialiasing fade factor
    let blur2 = (blur + (1.0 / pixel_ratio)) * gamma_scale;
    let denom = max(blur2, 1e-6);
    let alpha = clamp(min(dist - (in.v_width2.y - blur2), in.v_width2.x - dist) / denom, 0.0, 1.0);

    // Output non-premultiplied alpha: the blend state (SrcAlpha, OneMinusSrcAlpha)
    // handles the premultiplication. Using v_color * alpha here would double-apply alpha.
    // With a gradient the colour comes from the ramp and the layer's opacity from the alpha.
    // A stepped gradient's ramp covers the progress on a logarithmic scale.
    let stepped = dash_period.y > 1.2 && dash_period.y < 1.3;
    let clamped_progress = clamp(in.progress, 0.0, 1.0);
    let ramp_position = select(
        clamped_progress,
        log2(clamped_progress * 1048576.0 + 1.0) / 20.0,
        stepped,
    );
    let ramp = textureSample(ramp_texture, ramp_sampler, vec2<f32>(ramp_position, 0.5));
    // A pattern repeats along the line with its height fitted to the line's width.
    if dash_period.y > 1.5 {
        let width_px = max(in.dash.y, 1e-6);
        let along = fract(in.dash.x / dash_period.z * dash_period.w / (dash_period.y - 2.0));
        // The image spans the pattern less a texel of its own edge on every side, as GL JS
        // pads each image in its atlas.
        let dimensions = vec2<f32>(textureDimensions(ramp_texture));
        let uv = (vec2<f32>(along, 0.5 - 0.5 * in.across) * (dimensions + vec2<f32>(2.0)) - vec2<f32>(1.0)) / dimensions;
        let texel = textureSample(ramp_texture, ramp_sampler, uv);
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
