struct FragmentInput {
    @location(0) v_color: vec4<f32>,
    @location(1) v_stroke_color: vec4<f32>,
    @location(2) v_data: vec3<f32>,
    @location(3) v_params: vec4<f32>,
    @location(4) horizon_distance: f32,
};

struct Output {
    @location(0) out_color: vec4<f32>,
};

// smoothstep with the edges in either order, as the GL JS circle shader relies on.
fn smooth_edge(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = clamp((x - edge0) / (edge1 - edge0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

@fragment
fn main(in: FragmentInput) -> Output {
    if in.horizon_distance < 0.0 {
        discard;
    }
    let extrude_length = length(in.v_data.xy);
    let antialiased_blur = in.v_data.z;
    let radius_ratio = in.v_params.x;
    let opacity = in.v_params.y;
    let stroke_opacity = in.v_params.z;
    let stroke_width = in.v_params.w;

    let opacity_t = smooth_edge(0.0, antialiased_blur, extrude_length - 1.0);
    var color_t = 0.0;
    if stroke_width >= 0.01 {
        color_t = smooth_edge(antialiased_blur, 0.0, extrude_length - radius_ratio);
    }
    // Straight alpha: the blend state premultiplies, so only the coverage is folded in here.
    let rgb = mix(in.v_color.rgb, in.v_stroke_color.rgb, color_t);
    let alpha = mix(in.v_color.a * opacity, in.v_stroke_color.a * stroke_opacity, color_t) * opacity_t;
    if alpha < 0.5 / 255.0 {
        discard;
    }
    return Output(vec4<f32>(rgb, alpha));
}
