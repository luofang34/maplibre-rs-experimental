// @include ../../render/shaders/projection.vertex.wgsl

// Direction to the moon in globe world axes, then the tangent of its angular radius; the sun's
// direction, then 1 when a sun lights the phase; the globe transition, the moon's brightness
// relative to a full moon, and the halo's reach in disc radii.
struct MoonFrame {
    direction_radius: vec4<f32>,
    sun: vec4<f32>,
    transition_flux_reach: vec4<f32>,
};

@group(1) @binding(0) var<uniform> moon: MoonFrame;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    // Where around the disc the fragment lies, in disc radii along `right` and `up`.
    @location(0) offset: vec2<f32>,
    // The disc's axes in globe world axes, across it and along the line of sight.
    @location(1) @interpolate(flat) right: vec3<f32>,
    @location(2) @interpolate(flat) up: vec3<f32>,
    @location(3) @interpolate(flat) forward: vec3<f32>,
};

@vertex
fn main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0),
        vec2<f32>(-1.0, 1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
    );
    // The square reaches as far as the halo around the disc.
    let corner = corners[vertex_index] * moon.transition_flux_reach.z;
    let forward = moon.direction_radius.xyz;
    // Any axes across the line of sight will do: the shading uses the same ones.
    let reference = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0),
        abs(forward.y) > 0.9);
    let right = normalize(cross(forward, reference));
    let up = cross(right, forward);
    // The square lies on the sky around the moon's direction, so it turns with the camera and
    // stays at infinity, as the stars do.
    let corner_direction = forward + moon.direction_radius.w * (corner.x * right + corner.y * up);
    let clip = projection.main_matrix * vec4<f32>(corner_direction, 0.0);
    if clip.w <= 0.0 {
        return VertexOutput(vec4<f32>(2.0, 2.0, 2.0, 1.0), corner, right, up, forward);
    }
    // Depth zero is the reversed-Z far plane, behind everything that writes depth.
    return VertexOutput(vec4<f32>(clip.xy, 0.0, clip.w), corner, right, up, forward);
}
