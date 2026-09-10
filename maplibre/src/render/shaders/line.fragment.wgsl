@group(1) @binding(0) var dash_texture: texture_2d<f32>;
@group(1) @binding(1) var dash_sampler: sampler;
@group(1) @binding(2) var<uniform> dash_period: vec4<f32>;

struct FragmentInput {
    @location(0) v_color: vec4<f32>,
    @location(1) v_normal: vec2<f32>,
    @location(2) v_width2: vec2<f32>,
    @location(3) v_gamma_scale: f32,
    @location(4) horizon_distance: f32,
    @location(5) tile_x: f32,
    @location(6) @interpolate(flat) clip_antimeridian: u32,
    @location(7) dash: vec2<f32>,
};

struct Output {
    @location(0) out_color: vec4<f32>,
};

@fragment
fn main(in: FragmentInput) -> Output {
    let distance_sample = textureSample(dash_texture, dash_sampler,
        vec2<f32>(in.dash.x / max(dash_period.x, 1e-6), 0.5)).r;
    let dash_alpha = select(clamp(0.5 + (distance_sample * 255.0 - 128.0) / 254.0 * dash_period.x * in.dash.y, 0.0, 1.0),
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
    let blur = 0.0;
    
    // Calculate the antialiasing fade factor
    let blur2 = (blur + (1.0 / pixel_ratio)) * in.v_gamma_scale;
    let denom = max(blur2, 1e-6);
    let alpha = clamp(min(dist - (in.v_width2.y - blur2), in.v_width2.x - dist) / denom, 0.0, 1.0);

    // Output non-premultiplied alpha: the blend state (SrcAlpha, OneMinusSrcAlpha)
    // handles the premultiplication. Using v_color * alpha here would double-apply alpha.
    let coverage = in.v_color.a * alpha * dash_alpha;
    if coverage < 0.01 { discard; }
    return Output(vec4<f32>(in.v_color.rgb, coverage));
}
