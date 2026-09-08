struct VertexOutput {
    @location(0) @interpolate(flat) sky_color: vec4<f32>,
    @location(1) @interpolate(flat) horizon_color: vec4<f32>,
    @location(2) @interpolate(flat) horizon: vec4<f32>,
    @location(3) @interpolate(flat) blend: vec4<f32>,
    @builtin(position) position: vec4<f32>,
};

@fragment
fn main(in: VertexOutput) -> @location(0) vec4<f32> {
    // GL JS measures from the bottom of the screen.
    let x = in.position.x;
    let y = in.blend.z - in.position.y;
    let signed_distance = (y - in.horizon.y) * in.horizon.w + (x - in.horizon.x) * in.horizon.z;
    if signed_distance <= 0.0 && in.blend.w == 0.0 {
        discard;
    }
    // An external eye can see sky below the sea-level horizon through a valley.
    // Terrain depth hides this distant background wherever ground actually exists.
    let distance = max(signed_distance, 0.0);
    var color = in.sky_color;
    if distance < in.blend.x {
        color = mix(in.sky_color, in.horizon_color, pow(1.0 - distance / in.blend.x, 2.0));
    }
    let output = mix(color, vec4<f32>(0.0, 0.0, 0.0, 0.0), in.blend.y);
    if output.a <= 0.0 { discard; }
    return output;
}
