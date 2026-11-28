// @include atmosphere_scattering.wgsl.part

struct VertexInput {
    @location(0) view_direction: vec3<f32>,
    @location(1) @interpolate(flat) globe_position: vec3<f32>,
    @location(2) @interpolate(flat) sun_direction: vec4<f32>,
    @location(3) @interpolate(flat) globe_radius: f32,
    @location(4) @interpolate(flat) atmosphere_blend: f32,
    @location(5) @interpolate(flat) body_radius: f32,
};

@fragment
fn main(in: VertexInput) -> @location(0) vec4<f32> {
    let camera_relative_to_globe = -in.globe_position * in.body_radius / in.globe_radius;
    let raw_color = scattering(
        normalize(in.view_direction),
        camera_relative_to_globe,
        in.sun_direction.xyz,
        in.body_radius,
        in.sun_direction.w,
    );
    return displayed_scattering(raw_color) * in.atmosphere_blend;
}
