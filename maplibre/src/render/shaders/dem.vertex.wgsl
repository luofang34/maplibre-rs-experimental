// @include projection.vertex.wgsl

struct VertexOutput {
    @location(0) tex_coords: vec3<f32>,
    @location(1) horizon_distance: f32,
    // Unit Mercator y of the fragment, for the latitude the slope scale needs.
    @location(2) mercator_y: f32,
    // Zoom of the tile the texture belongs to.
    @location(3) zoom: f32,
    @builtin(position) clip_position: vec4<f32>,
};

const EXTENT: f32 = 4096.0;

@vertex
fn main(
    @location(0) raw_position: vec2<i32>,
    @location(2) tile_mercator_coords: vec4<f32>,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
    @location(9) zoom_factor: f32,

    @location(10) z_index: f32,

) -> VertexOutput {
    let tile_position = vec3<f32>(vec2<f32>(raw_position), 0.0);
    let projected = project_tile_mesh_position(
        tile_position,
        raw_position,
        mat4x4<f32>(translate1, translate2, translate3, translate4),
        tile_mercator_coords,
    );
    var tex_coords = vec2<f32>(raw_position) / EXTENT;
    if raw_position.y < -32767 {
        tex_coords.y = 0.0;
    } else if raw_position.y > 32766 {
        tex_coords.y = 1.0;
    }
    let mercator_y = tile_mercator_coords.y + tex_coords.y * EXTENT * tile_mercator_coords.w;
    let zoom = -log2(EXTENT * tile_mercator_coords.z);
    return VertexOutput(
        vec3<f32>(tex_coords, 1.0),
        projected.horizon_distance,
        mercator_y,
        zoom,
        projected.clip_position,
    );
}
