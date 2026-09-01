// @include projection.vertex.wgsl

struct VertexOutput {
    @location(0) surface: vec3<f32>,
    @location(1) surface_distance: f32,
    @location(2) atmosphere_blend: f32,
    @location(3) view_direction: vec3<f32>,
    @builtin(position) position: vec4<f32>,
};

@vertex
fn main(
    @location(0) raw_position: vec2<i32>,
    @location(2) tile_mercator_coords: vec4<f32>,
    @location(8) atmosphere_blend: f32,
) -> VertexOutput {
    let tile_position = vec2<f32>(raw_position);
    let surface = tile_position_on_unit_sphere(tile_position, tile_mercator_coords);
    let shell_position = surface * 1.0157;
    let clip_position = projection.main_matrix * vec4<f32>(shell_position, 1.0);
    let surface_distance = dot(projection.clipping_plane, vec4<f32>(surface, 1.0));
    return VertexOutput(
        surface,
        surface_distance,
        atmosphere_blend,
        normalize(projection.clipping_plane.xyz),
        clip_position,
    );
}
