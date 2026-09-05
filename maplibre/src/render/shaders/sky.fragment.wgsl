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
    let distance = (y - in.horizon.y) * in.horizon.w + (x - in.horizon.x) * in.horizon.z;
    if distance <= 0.0 {
        // Below the horizon the map covers everything.
        discard;
    }
    var color = in.sky_color;
    if distance < in.blend.x {
        color = mix(in.sky_color, in.horizon_color, pow(1.0 - distance / in.blend.x, 2.0));
    }
    return mix(color, vec4<f32>(0.0, 0.0, 0.0, 0.0), in.blend.y);
}
