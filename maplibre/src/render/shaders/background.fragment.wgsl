struct Output {
    @location(0) out_color: vec4<f32>,
};

@fragment
fn main(
    @location(0) v_color: vec4<f32>,
    @location(1) @interpolate(flat) horizon: vec4<f32>,
    @location(2) @interpolate(flat) viewport: vec4<f32>,
    @location(3) @interpolate(flat) row_numerator: vec4<f32>,
    @location(4) @interpolate(flat) row_denominator: vec4<f32>,
    @builtin(position) position: vec4<f32>,
) -> Output {
    // GL JS draws the background on the tiles of the flat map, which end at the horizon;
    // above it the sky or nothing shows. GL JS measures from the bottom of the screen.
    let y = viewport.x - position.y;
    let distance = (y - horizon.y) * horizon.w + (position.x - horizon.x) * horizon.z;
    if distance > 0.0 {
        discard;
    }
    // The flat map is the tiles of the world, which end at its northern and southern edges.
    let ndc = vec3<f32>(
        position.x / viewport.z * 2.0 - 1.0,
        1.0 - position.y / viewport.w * 2.0,
        1.0,
    );
    let row = dot(row_numerator.xyz, ndc) / dot(row_denominator.xyz, ndc);
    if row < row_numerator.w || row > row_denominator.w {
        discard;
    }
    return Output(vec4<f32>(v_color.rgb * v_color.a, v_color.a));
}
