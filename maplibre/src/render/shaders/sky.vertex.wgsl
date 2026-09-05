struct VertexOutput {
    @location(0) @interpolate(flat) sky_color: vec4<f32>,
    @location(1) @interpolate(flat) horizon_color: vec4<f32>,
    // Horizon point on screen in GL coordinates (y up) and the normal pointing into the sky.
    @location(2) @interpolate(flat) horizon: vec4<f32>,
    // Sky-horizon blend width in pixels, the globe transition, the viewport height.
    @location(3) @interpolate(flat) blend: vec4<f32>,
    @builtin(position) position: vec4<f32>,
};

@vertex
fn main(
    @builtin(vertex_index) vertex_index: u32,
    @location(8) sky_color: vec4<f32>,
    @location(9) horizon_color: vec4<f32>,
    @location(10) horizon: vec4<f32>,
    @location(11) blend: vec4<f32>,
) -> VertexOutput {
    var position = vec2<f32>(-1.0, -1.0);
    if vertex_index == 1u {
        position = vec2<f32>(3.0, -1.0);
    } else if vertex_index == 2u {
        position = vec2<f32>(-1.0, 3.0);
    }
    return VertexOutput(sky_color, horizon_color, horizon, blend, vec4<f32>(position, 0.0, 1.0));
}
