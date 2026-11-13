// @include projection.vertex.wgsl

struct VertexOutput {
    @location(0) tex_coords: vec3<f32>,
    @location(1) horizon_distance: f32,
    // Unit Mercator y of the fragment, for the latitude the slope scale needs.
    @location(2) mercator_y: f32,
    // Zoom of the tile the texture belongs to.
    @location(3) zoom: f32,
    // How far the view has moved from the Mercator plane to the globe.
    @location(4) transition: f32,
    // Signed polar-cap progress (+1 at 90°N, -1 at 90°S, 0 on tile body).
    @location(5) polar_cap: f32,
    @builtin(position) clip_position: vec4<f32>,
};

const EXTENT: f32 = 4096.0;

@vertex
fn main(
    @location(0) raw_position: vec2<f32>,
    @location(2) tile_mercator_coords: vec4<f32>,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
    @location(9) overlap: f32,
) -> VertexOutput {
    // A tile drawn into a drape texture reaches a hair past its edges, which extend the texels
    // there instead of stretching the tile.
    let is_north = is_north_pole_y(raw_position.y);
    let is_south = is_south_pole_y(raw_position.y);
    let is_pole = is_north || is_south;
    let enlarged = (raw_position - vec2<f32>(EXTENT * 0.5)) * overlap + vec2<f32>(EXTENT * 0.5);
    let tile_position = vec3<f32>(select(enlarged, raw_position, is_pole), 0.0);
    let projected = project_tile_mesh_position(
        tile_position,
        raw_position,
        mat4x4<f32>(translate1, translate2, translate3, translate4),
        tile_mercator_coords,
    );
    var tex_coords = tile_position.xy / EXTENT;
    var polar_cap = 0.0;
    if is_north {
        tex_coords.y = 0.0;
        polar_cap = 1.0;
    } else if is_south {
        tex_coords.y = 1.0;
        polar_cap = -1.0;
    }
    let mercator_y = tile_mercator_coords.y + tex_coords.y * EXTENT * tile_mercator_coords.w;
    let zoom = -log2(EXTENT * tile_mercator_coords.z);
    return VertexOutput(
        vec3<f32>(tex_coords, 1.0),
        projected.horizon_distance,
        mercator_y,
        zoom,
        projection.transition_and_padding.x,
        polar_cap,
        projected.clip_position,
    );
}
