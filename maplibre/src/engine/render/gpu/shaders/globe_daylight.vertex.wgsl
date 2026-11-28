// The frame's view rays across a triangle covering the screen at the far plane, with the
// globe and the host's sky in view axes.

struct VertexOutput {
    @location(0) view_direction: vec3<f32>,
    @location(1) @interpolate(flat) globe: vec4<f32>,
    @location(2) @interpolate(flat) sun: vec3<f32>,
    @location(3) @interpolate(flat) twilight: vec4<f32>,
    @location(4) @interpolate(flat) moonlight: vec4<f32>,
    @builtin(position) position: vec4<f32>,
};

@vertex
fn main(
    @builtin(vertex_index) vertex_index: u32,
    @location(0) inverse_projection_0: vec4<f32>,
    @location(1) inverse_projection_1: vec4<f32>,
    @location(2) inverse_projection_2: vec4<f32>,
    @location(3) inverse_projection_3: vec4<f32>,
    @location(4) globe: vec4<f32>,
    @location(5) sun: vec4<f32>,
    @location(6) twilight: vec4<f32>,
    @location(7) moonlight: vec4<f32>,
) -> VertexOutput {
    var position = vec2<f32>(-1.0, -1.0);
    if vertex_index == 1u {
        position = vec2<f32>(3.0, -1.0);
    } else if vertex_index == 2u {
        position = vec2<f32>(-1.0, 3.0);
    }
    // Reversed depth puts the far plane at zero, behind everything the main pass drew.
    let clip_position = vec4<f32>(position, 0.0, 1.0);
    let inverse_projection = mat4x4<f32>(
        inverse_projection_0,
        inverse_projection_1,
        inverse_projection_2,
        inverse_projection_3,
    );
    let view = inverse_projection * clip_position;
    return VertexOutput(view.xyz, globe, sun.xyz, twilight, moonlight, clip_position);
}
