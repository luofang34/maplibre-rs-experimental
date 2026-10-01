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
// The Gaussian's normalisation, 1 / sqrt(2 * pi).
const GAUSS_COEF: f32 = 0.3989422804014327;
// Density below this is not drawn: an eighth of the smallest step of an 8-bit ramp lookup.
const ZERO: f32 = 1.0 / 255.0 / 16.0;

@vertex
fn main(
    @builtin(vertex_index) vertex_index: u32,
    @location(0) position: vec2<f32>,
    // Radius factor, weight, path distance and, over terrain, the height of the point.
    @location(1) path: vec4<f32>,
    @location(2) tile_mercator_coords: vec4<f32>,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
    @location(9) zoom_factor: f32,
    @location(13) layer_radius: f32,
    @location(14) circle_params: vec4<f32>,
) -> VertexOutput {
    // Quads are emitted with four consecutive vertices; the corner follows the vertex order.
    let corner = vertex_index % 4u;
    let extrude = vec2<f32>(
        select(-1.0, 1.0, corner == 1u || corner == 2u),
        select(-1.0, 1.0, corner >= 2u),
    );
    // One unit for a layer-wide radius, else the point's own radius in pixels.
    let radius = layer_radius * path.x;
    let weight = path.y;
    let intensity = circle_params.x;
    let strength = weight * intensity;
    // Extend the quad until the kernel falls below ZERO, so a strong point has no visible edge.
    let peak = max(strength * GAUSS_COEF, ZERO * 1.0001);
    let scale = sqrt(-2.0 * log(ZERO / peak)) / 3.0;
    let reach = extrude * scale;
    let transform = mat4x4<f32>(translate1, translate2, translate3, translate4);
    // The radius is in pixels on the map plane, so pitch shrinks far points as it does circles.
    let corner_position = position + reach * radius * TILE_UNITS_PER_PIXEL * zoom_factor;
    // A point over terrain stands on the surface, as GL JS places its kernels.
    let spatial = path.w > -1e20;
    var projected: ProjectedTilePosition;
    if spatial {
        projected = project_tile_position_3d(
            vec3<f32>(corner_position, path.w),
            transform,
            tile_mercator_coords,
        );
    } else {
        projected = project_tile_position(
            vec3<f32>(corner_position, 0.0),
            transform,
            tile_mercator_coords,
        );
    }
    var clip = projected.clip_position;
    clip.z = 0.0;
    return VertexOutput(clip, reach, strength, projected.horizon_distance);
}
