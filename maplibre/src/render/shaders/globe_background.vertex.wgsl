// @include projection.vertex.wgsl

struct VertexOutput {
    @location(0) color: vec4<f32>,
    @location(4) horizon_distance: f32,
    @builtin(position) position: vec4<f32>,
};

@vertex
fn main(
    @location(0) raw_position: vec2<i32>,
    @location(2) tile_mercator_coords: vec4<f32>,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
    @location(8) color: vec4<f32>,
    @location(10) z_index: f32,
    @location(11) viewport: vec4<f32>,
) -> VertexOutput {
    let tile_position = vec3<f32>(vec2<f32>(raw_position), 0.0);
    let projected = project_tile_position_3d(
        tile_position,
        mat4x4<f32>(translate1, translate2, translate3, translate4),
        tile_mercator_coords,
    );
    var position = projected.clip_position;
    // Background paint cannot occlude bathymetry or land below the reference ellipsoid.
    // Keep a positive compositor depth, behind terrain and ahead of the sky.
    if viewport.y > 0.5 {
        position.z = 1.0e-7 * position.w;
    }
    return VertexOutput(color, projected.horizon_distance, position);
}
