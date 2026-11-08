struct VertexOutput {
    @location(0) @interpolate(flat) sky_color: vec4<f32>,
    @location(1) @interpolate(flat) horizon_color: vec4<f32>,
    // Horizon point on screen in GL coordinates (y up) and the normal pointing into the sky.
    @location(2) @interpolate(flat) horizon: vec4<f32>,
    // Sky-horizon blend width, the globe transition, the viewport height (negative for the
    // globe sky), and sky depth.
    @location(3) @interpolate(flat) blend: vec4<f32>,
    // View-space ray through the pixel, for the globe sky.
    @location(4) view_direction: vec3<f32>,
    // The eye's zenith in view axes and the elevation of its sea-level horizon in radians,
    // or the globe center in view pixels when blending the Mercator sky onto a globe.
    @location(5) @interpolate(flat) up_and_horizon: vec4<f32>,
    // Camera-to-center distance, device pixel ratio, globe radius in pixels, and atmosphere blend.
    @location(6) @interpolate(flat) globe_params: vec4<f32>,
    @builtin(position) position: vec4<f32>,
};

@vertex
fn main(
    @builtin(vertex_index) vertex_index: u32,
    @location(8) sky_color: vec4<f32>,
    @location(9) horizon_color: vec4<f32>,
    @location(10) horizon: vec4<f32>,
    @location(11) blend: vec4<f32>,
    @location(12) inverse_projection_0: vec4<f32>,
    @location(13) inverse_projection_1: vec4<f32>,
    @location(14) inverse_projection_2: vec4<f32>,
    @location(15) inverse_projection_3: vec4<f32>,
    @location(7) up_and_horizon: vec4<f32>,
) -> VertexOutput {
    var position = vec2<f32>(-1.0, -1.0);
    if vertex_index == 1u {
        position = vec2<f32>(3.0, -1.0);
    } else if vertex_index == 2u {
        position = vec2<f32>(-1.0, 3.0);
    }
    // As for the atmosphere, the inverse perspective already returns the ray vector;
    // dividing by W would only lose precision near the far plane. Since clip Z is 0,
    // column 2 of the inverse projection does not contribute to the ray and carries
    // `globe_params` for the fragment shader instead.
    let view = inverse_projection_0 * position.x
        + inverse_projection_1 * position.y
        + inverse_projection_3;
    return VertexOutput(
        sky_color,
        horizon_color,
        horizon,
        blend,
        view.xyz,
        up_and_horizon,
        inverse_projection_2,
        vec4<f32>(position, blend.w, 1.0),
    );
}
