// @include projection.vertex.wgsl

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    // Extrusion direction: the kernel is a function of the distance from the centre.
    @location(0) extrude: vec2<f32>,
    // Weight times intensity.
    @location(1) strength: f32,
    @location(2) horizon_distance: f32,
};

// A 512-pixel view tile spans the 4096-unit tile grid.
const TILE_UNITS_PER_PIXEL: f32 = 8.0;
// The kernel reaches three standard deviations, so one radius covers the whole bell.
const KERNEL_EXTENT: f32 = 1.0;

@vertex
fn main(
    @builtin(vertex_index) vertex_index: u32,
    @location(0) position: vec2<f32>,
    @location(1) normal: vec2<f32>,
    @location(2) tile_mercator_coords: vec4<f32>,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
    @location(9) zoom_factor: f32,
    @location(13) radius: f32,
    @location(14) circle_params: vec4<f32>,
) -> VertexOutput {
    // Quads are emitted with four consecutive vertices; the corner follows the vertex order.
    let corner = vertex_index % 4u;
    let extrude = vec2<f32>(
        select(-1.0, 1.0, corner == 1u || corner == 2u),
        select(-1.0, 1.0, corner >= 2u),
    );
    let weight = normal.y;
    let intensity = circle_params.x;
    let transform = mat4x4<f32>(translate1, translate2, translate3, translate4);
    // The radius is in pixels on the map plane, so pitch shrinks far points as it does circles.
    let corner_position = position + extrude * radius * KERNEL_EXTENT * TILE_UNITS_PER_PIXEL * zoom_factor;
    let projected = project_tile_position(
        vec3<f32>(corner_position, 0.0),
        transform,
        tile_mercator_coords,
    );
    var clip = projected.clip_position;
    clip.z = 0.0;
    return VertexOutput(clip, extrude, weight * intensity, projected.horizon_distance);
}
