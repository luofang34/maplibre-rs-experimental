struct Output {
    @location(0) out_color: vec4<f32>,
};

@fragment
fn main(
    @location(0) v_color: vec4<f32>,
    @location(1) @interpolate(flat) horizon: vec4<f32>,
    @location(2) @interpolate(flat) viewport: vec4<f32>,
    @builtin(position) position: vec4<f32>,
) -> Output {
    // GL JS draws the background on the tiles of the flat map, which end at the horizon;
    // above it the sky or nothing shows. GL JS measures from the bottom of the screen.
    let y = viewport.x - position.y;
    let distance = (y - horizon.y) * horizon.w + (position.x - horizon.x) * horizon.z;
    if distance > 0.0 {
        discard;
    }
    return Output(v_color);
}
