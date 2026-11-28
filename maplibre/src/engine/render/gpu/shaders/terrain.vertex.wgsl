// @include projection.vertex.wgsl
// @include terrain_surface.wgsl.part

struct VertexOutput {
    @location(0) tex_coords: vec2<f32>,
    // Distance along the view axis, the clip w, from which the fragment takes its fog.
    @location(1) eye_depth: f32,
    @location(2) surface_normal: vec3<f32>,
    @location(3) camera_relative_position: vec3<f32>,
    // 1 at a pole and 0 on the tile, so the polar cap's triangles are told apart from the
    // tile's: their texture coordinates would only repeat the tile's edge row to the pole.
    @location(4) polar_cap: f32,
    // The ground's geographic up in globe world axes, which the sun's elevation is measured
    // from on the globe and the flat map alike.
    @location(5) globe_normal: vec3<f32>,
    // Unskirted stitched terrain elevation in metres MSL for procedural relief & TAWS bands.
    @location(6) surface_elevation: f32,
    @builtin(position) clip_position: vec4<f32>,
};

@group(1) @binding(0) var<uniform> terrain_tile: TerrainTileUniforms;
@group(1) @binding(1) var dem_texture: texture_2d<f32>;
@group(1) @binding(4) var edge_heights: texture_2d<f32>;

const POLAR_CAP_COLATITUDE: f32 = 0.086374097;

fn polar_cap_unit_sphere(
    raw_y: i32,
    position_x: f32,
    north_cap: bool,
    cap_t: f32,
) -> vec3<f32> {
    if raw_y == -32768 {
        return vec3<f32>(0.0, 1.0, 0.0);
    }
    if raw_y == 32767 {
        return vec3<f32>(0.0, -1.0, 0.0);
    }
    let rem = 1.0 - cap_t;
    let theta = POLAR_CAP_COLATITUDE * rem * rem;
    let merc_x = terrain_tile.tile_mercator_coords.x + position_x * terrain_tile.tile_mercator_coords.z;
    let lon = merc_x * PROJECTION_TWO_PI + PROJECTION_PI;
    return vec3<f32>(
        sin(lon) * sin(theta),
        select(-cos(theta), cos(theta), north_cap),
        cos(lon) * sin(theta),
    );
}

fn polar_cap_clip(
    raw_y: i32,
    position: vec2<f32>,
    elevation: f32,
    north_cap: bool,
    cap_t: f32,
    cap_surface: vec3<f32>,
) -> vec4<f32> {
    let height_radii = elevation / projection.transition_and_padding.z;
    var globe_clip: vec4<f32>;
    if projection.globe_circle.y >= 0.5 {
        let center = projection.globe_center.xy;
        let lon0 = center.x * PROJECTION_TWO_PI + PROJECTION_PI;
        let t0 = exp(PROJECTION_PI - center.y * PROJECTION_TWO_PI);
        let denom0 = t0 * t0 + 1.0;
        let sin_lat0 = (t0 * t0 - 1.0) / denom0;
        let cos_lat0 = 2.0 * t0 / denom0;
        let sin_lon0 = sin(lon0);
        let cos_lon0 = cos(lon0);
        var sphere_delta: vec3<f32>;
        if raw_y == -32768 {
            sphere_delta = vec3<f32>(-sin_lon0 * cos_lat0, 2.0 / denom0, -cos_lon0 * cos_lat0);
        } else if raw_y == 32767 {
            sphere_delta = vec3<f32>(
                -sin_lon0 * cos_lat0,
                -2.0 * t0 * t0 / denom0,
                -cos_lon0 * cos_lat0,
            );
        } else {
            let rem = 1.0 - cap_t;
            let theta = POLAR_CAP_COLATITUDE * rem * rem;
            let d_lon = ((terrain_tile.tile_mercator_coords.x - center.x)
                + position.x * terrain_tile.tile_mercator_coords.z) * PROJECTION_TWO_PI;
            let d_lat = select(theta - 2.0 * atan(t0), 2.0 * atan(1.0 / t0) - theta, north_cap);
            let half_lat = sin(0.5 * d_lat);
            let versine_lat = -2.0 * half_lat * half_lat;
            let d_sin_lat = sin_lat0 * versine_lat + cos_lat0 * sin(d_lat);
            let d_cos_lat = cos_lat0 * versine_lat - sin_lat0 * sin(d_lat);
            let half_lon = sin(0.5 * d_lon);
            let versine_lon = -2.0 * half_lon * half_lon;
            let d_sin_lon = sin_lon0 * versine_lon + cos_lon0 * sin(d_lon);
            let d_cos_lon = cos_lon0 * versine_lon - sin_lon0 * sin(d_lon);
            sphere_delta = vec3<f32>(
                sin_lon0 * d_cos_lat + d_sin_lon * cos_lat0 + d_sin_lon * d_cos_lat,
                d_sin_lat,
                cos_lon0 * d_cos_lat + d_cos_lon * cos_lat0 + d_cos_lon * d_cos_lat,
            );
        }
        let offset = projection.main_matrix * vec4<f32>(sphere_delta, 0.0);
        globe_clip = projection.globe_center_clip + offset
            + (projection.globe_center_radial + offset) * height_radii;
    } else {
        globe_clip = projection.main_matrix * vec4<f32>(cap_surface * (1.0 + height_radii), 1.0);
    }
    let flat = terrain_tile.transform * vec4<f32>(vec2<f32>(position.x, f32(raw_y)), elevation, 1.0);
    return blend_clip(flat, globe_clip, projection.transition_and_padding.x);
}

// Small Mercator offsets avoid quantizing global longitude and sphere positions to metres.
fn tile_relative_globe(position: vec2<f32>, elevation: f32) -> vec4<f32> {
    let origin = terrain_tile.globe_origin;
    let angle = (position - vec2<f32>(TERRAIN_EXTENT * 0.5)) * origin.z;
    let y2 = angle.y * angle.y;
    let sh = angle.y * (1.0 + y2 / 6.0 + y2 * y2 / 120.0);
    let ch_minus_one = y2 * (0.5 + y2 / 24.0 + y2 * y2 / 720.0);
    let denominator = 1.0 + ch_minus_one - origin.x * sh;
    let ds = -origin.y * origin.y * sh / denominator;
    let dc = origin.y * (origin.x * sh - ch_minus_one) / denominator;
    let cosine = origin.y + dc;
    let half_sine = sin(angle.x * 0.5);
    let longitude_sag = 2.0 * cosine * half_sine * half_sine;
    let delta = vec3<f32>(cosine * sin(angle.x),
        origin.y * ds - origin.x * (dc - longitude_sag),
        -0.5 * (ds * ds + dc * dc) - origin.y * longitude_sag);
    let metres = delta * (origin.w + elevation) + vec3<f32>(0.0, 0.0, elevation);
    return terrain_tile.globe_transform * vec4<f32>(metres, 1.0);
}

@vertex
fn main(
    @location(0) raw_position: vec2<i32>,
    @location(1) skirt: vec2<u32>,
) -> VertexOutput {
    let position = clamp(vec2<f32>(raw_position), vec2<f32>(0.0), vec2<f32>(TERRAIN_EXTENT));
    var surface_position_2d = position;
    let caps_enabled = projection.transition_and_padding.x > 0.0;
    let north_cap = caps_enabled && raw_position.y < 0 && terrain_tile.tile_mercator_coords.y == 0.0;
    let south_cap = caps_enabled && raw_position.y > 4096 &&
        terrain_tile.tile_mercator_coords.y + TERRAIN_EXTENT * terrain_tile.tile_mercator_coords.w >= 1.0;
    let is_cap = north_cap || south_cap;
    var cap_t = 0.0;
    if north_cap {
        cap_t = clamp(-f32(raw_position.y) / 32768.0, 0.0, 1.0);
    } else if south_cap {
        cap_t = clamp((f32(raw_position.y) - TERRAIN_EXTENT) / (32767.0 - TERRAIN_EXTENT), 0.0, 1.0);
    }
    let cap_surface = polar_cap_unit_sphere(raw_position.y, position.x, north_cap, cap_t);
    if is_cap { surface_position_2d.y = f32(raw_position.y); }
    // Every longitude meets at the same altitude; unknown polar DEM samples cannot split the fan.
    let height_gradient = terrain_height_gradient(position);
    let edge_stitched = stitched_elevation(position, height_gradient.x);
    let pole_elevation = bitcast<f32>(terrain_tile.edge_index.w);
    let cap_rem = 1.0 - cap_t;
    let radial_t = 1.0 - cap_rem * cap_rem;
    let cap_blend = smoothstep(0.0, 0.35, radial_t);
    let stitched = select(edge_stitched, mix(edge_stitched, pole_elevation, cap_blend), is_cap);
    let elevation = stitched - f32(skirt.x) * terrain_tile.skirt_length;
    let msl_elevation = stitched / max(terrain_tile.exaggeration, 1e-4);
    var clip_position: vec4<f32>;
    if is_cap {
        clip_position = polar_cap_clip(raw_position.y, position, elevation, north_cap, cap_t, cap_surface);
    } else {
        var projected = project_tile_position_3d(
            vec3<f32>(surface_position_2d, elevation),
            terrain_tile.transform,
            terrain_tile.tile_mercator_coords,
        );
        if terrain_tile.globe_origin.w > 0.0 {
            let flat = terrain_tile.transform * vec4<f32>(surface_position_2d, elevation, 1.0);
            projected.clip_position = blend_clip(flat, tile_relative_globe(position, elevation),
                projection.transition_and_padding.x);
        }
        clip_position = projected.clip_position;
    }
    // The fog follows from the eye depth per fragment: it is not linear in depth, and the
    // triangles of a far tile span hundreds of kilometres, so a value found per vertex
    // would step at every tile edge.
    // Tile-relative metres keep slope lighting independent of zoom and eye orientation.
    let metres_per_unit = PROJECTION_TWO_PI * projection.transition_and_padding.z
        * terrain_tile.tile_mercator_coords.z
        * globe_circumference_ratio_at_tile_y(TERRAIN_EXTENT * 0.5, terrain_tile.tile_mercator_coords);
    let edge_normal = normalize(vec3<f32>(-height_gradient.y, height_gradient.z, metres_per_unit));
    let normal = select(
        edge_normal,
        normalize(mix(edge_normal, vec3<f32>(0.0, 0.0, 1.0), cap_blend)),
        is_cap,
    );
    let globe_normal = select(
        tile_position_on_unit_sphere(surface_position_2d, terrain_tile.tile_mercator_coords),
        cap_surface,
        is_cap,
    );
    return VertexOutput(
        position / TERRAIN_EXTENT,
        clip_position.w,
        normal,
        terrain_tile.fog_position.xyz + vec3<f32>(position.x * terrain_tile.fog_position.w,
            -position.y * terrain_tile.fog_position.w, elevation),
        select(0.0, 1.0, is_cap),
        globe_normal,
        msl_elevation,
        clip_position,
    );
}
