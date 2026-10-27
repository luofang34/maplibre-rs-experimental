// @include ../render/shaders/projection.vertex.wgsl

// x, y: the viewport in physical pixels; z: physical pixels per logical pixel; w: how far the
// map has turned into a globe, which the stars fade in with.
struct StarfieldFrame {
    viewport_ratio_transition: vec4<f32>,
};

@group(1) @binding(0) var<uniform> starfield: StarfieldFrame;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    // The corner of the star's square, from -1 to 1 across it.
    @location(0) corner: vec2<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
};

@vertex
fn main(
    @builtin(vertex_index) vertex_index: u32,
    @location(0) direction_radius: vec4<f32>,
    @location(1) color_brightness: vec4<f32>,
) -> VertexOutput {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0),
        vec2<f32>(-1.0, 1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
    );
    let corner = corners[vertex_index];
    // A direction has no position, so the view's translation drops out and its scale cancels
    // in the perspective divide: the star turns with the camera and stays at infinity.
    let clip = projection.main_matrix * vec4<f32>(direction_radius.xyz, 0.0);
    let frame = starfield.viewport_ratio_transition;
    if clip.w <= 0.0 {
        // Behind the eye: outside every clip volume.
        return VertexOutput(vec4<f32>(2.0, 2.0, 2.0, 1.0), corner, vec4<f32>(0.0));
    }
    let radius = direction_radius.w * frame.z;
    let offset = corner * radius * 2.0 / frame.xy;
    // Depth zero is the reversed-Z far plane, behind everything that writes depth.
    let position = vec4<f32>(clip.xy + offset * clip.w, 0.0, clip.w);
    return VertexOutput(position, corner,
        vec4<f32>(color_brightness.rgb, color_brightness.a * frame.w));
}
