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
    @location(0) v_pos_q2: vec2<i32>,
    @location(1) v_norm_packed: u32,
    @location(10) v_extra: vec2<u32>,
    @location(2) tile_mercator_coords: vec4<f32>,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
    @location(9) zoom_factor: f32,
    @location(13) layer_radius: f32,
    @location(14) circle_params: vec4<f32>,
) -> VertexOutput {
    let position = vec2<f32>(v_pos_q2) * 0.25 + vec2<f32>(2048.0);
    let norm = unpack2x16float(v_norm_packed);
    let elev_in = unpack2x16float(v_extra.y & 0xffffu).x;
    let corner_cell = (v_extra.y >> 16u) & 0x7fu;
    // Quads are emitted with four consecutive vertices; the corner follows the vertex order.
    let corner = vertex_index % 4u;
    var extrude = vec2<f32>(
        select(-1.0, 1.0, corner == 1u || corner == 2u),
        select(-1.0, 1.0, corner >= 2u),
    );
    // A subdivided kernel names its corner on an eight-by-eight grid, to follow the globe.
    if corner_cell > 0u {
        let cell = corner_cell - 1u;
        extrude = vec2<f32>(f32(cell / 8u), f32(cell % 8u)) / 7.0 * 2.0 - vec2<f32>(1.0);
    }
    // One unit for a layer-wide radius, else the point's own radius in pixels.
    let radius = layer_radius * norm.x;
    let weight = norm.y;
    let intensity = circle_params.x;
    let strength = weight * intensity;
    // Extend the quad until the kernel falls below ZERO, so a strong point has no visible edge.
    let peak = max(strength * GAUSS_COEF, ZERO * 1.0001);
    let scale = sqrt(-2.0 * log(ZERO / peak)) / 3.0;
    let reach = extrude * scale;
    let transform = mat4x4<f32>(translate1, translate2, translate3, translate4);
    // The radius is in pixels on the map plane, so pitch shrinks far points as it does circles.
    let corner_offset = reach * radius * TILE_UNITS_PER_PIXEL * zoom_factor;
    let corner_position = position + corner_offset;
    // A point over terrain stands on the surface, as GL JS places its kernels.
    let spatial = elev_in > -30000.0;
    var projected: ProjectedTilePosition;
    let transition = projection.transition_and_padding.x;
    if transition > 0.0 {
        // On the globe the kernel keeps its angular size, as a map-aligned circle does, and
        // stands on the sphere whatever the terrain below it.
        let center = project_tile_position(vec3<f32>(position, 0.0), transform, tile_mercator_coords);
        let center_vector = tile_position_on_unit_sphere(position, tile_mercator_coords);
        let angles = corner_offset * tile_mercator_coords.z * PROJECTION_TWO_PI * projection.globe_circle.x;
        let globe_clip = projection.main_matrix * vec4<f32>(globe_rotate_vector(center_vector, angles), 1.0);
        projected = ProjectedTilePosition(
            interpolate_clip_position(
                transform * vec4<f32>(corner_position, 0.0, 1.0),
                globe_clip,
                transition,
            ),
            center.horizon_distance,
        );
    } else if spatial {
        projected = project_tile_position_3d(
            vec3<f32>(corner_position, elev_in),
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
