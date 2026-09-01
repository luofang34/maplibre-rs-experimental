struct ShaderProjectionData {
    main_matrix: mat4x4<f32>,
    clipping_plane: vec4<f32>,
    transition_and_padding: vec4<f32>,
};

struct ProjectedTilePosition {
    clip_position: vec4<f32>,
    horizon_distance: f32,
};

@group(0) @binding(0) var<uniform> projection: ShaderProjectionData;

const PROJECTION_PI: f32 = 3.141592653589793;
const PROJECTION_TWO_PI: f32 = 6.283185307179586;

fn tile_position_on_unit_sphere(
    tile_position: vec2<f32>,
    tile_mercator_coords: vec4<f32>,
) -> vec3<f32> {
    let mercator = tile_mercator_coords.xy + tile_position * tile_mercator_coords.zw;
    let longitude = mercator.x * PROJECTION_TWO_PI + PROJECTION_PI;
    let tangent_half_latitude = exp(PROJECTION_PI - mercator.y * PROJECTION_TWO_PI);
    let tangent_half_latitude_squared = tangent_half_latitude * tangent_half_latitude;
    let denominator = tangent_half_latitude_squared + 1.0;
    let sin_latitude = (tangent_half_latitude_squared - 1.0) / denominator;
    let cos_latitude = 2.0 * tangent_half_latitude / denominator;
    return vec3<f32>(
        sin(longitude) * cos_latitude,
        sin_latitude,
        cos(longitude) * cos_latitude,
    );
}

fn project_tile_position(
    tile_position: vec3<f32>,
    fallback_matrix: mat4x4<f32>,
    tile_mercator_coords: vec4<f32>,
) -> ProjectedTilePosition {
    let transition = projection.transition_and_padding.x;
    let surface = tile_position_on_unit_sphere(tile_position.xy, tile_mercator_coords);
    let mercator_clip = fallback_matrix * vec4<f32>(tile_position, 1.0);
    let globe_clip = projection.main_matrix * vec4<f32>(surface, 1.0);
    let horizon_distance = mix(
        1.0,
        dot(projection.clipping_plane, vec4<f32>(surface, 1.0)),
        transition,
    );
    return ProjectedTilePosition(
        mix(mercator_clip, globe_clip, transition),
        horizon_distance,
    );
}

fn project_tile_mesh_position(
    tile_position: vec3<f32>,
    raw_position: vec2<i32>,
    fallback_matrix: mat4x4<f32>,
    tile_mercator_coords: vec4<f32>,
) -> ProjectedTilePosition {
    var surface = tile_position_on_unit_sphere(tile_position.xy, tile_mercator_coords);
    if raw_position.y < -32767 {
        surface = vec3<f32>(0.0, 1.0, 0.0);
    } else if raw_position.y > 32766 {
        surface = vec3<f32>(0.0, -1.0, 0.0);
    }
    let transition = projection.transition_and_padding.x;
    let mercator_clip = fallback_matrix * vec4<f32>(tile_position, 1.0);
    let globe_clip = projection.main_matrix * vec4<f32>(surface, 1.0);
    let horizon_distance = mix(
        1.0,
        dot(projection.clipping_plane, vec4<f32>(surface, 1.0)),
        transition,
    );
    return ProjectedTilePosition(
        mix(mercator_clip, globe_clip, transition),
        horizon_distance,
    );
}
