// @include projection.vertex.wgsl

struct TerrainTileUniforms {
    transform: mat4x4<f32>,
    dem_matrix: mat4x4<f32>,
    drape_matrix: mat4x4<f32>,
    tile_mercator_coords: vec4<f32>,
    dem_unpack: vec4<f32>,
    dem_dim: f32,
    exaggeration: f32,
    skirt_length: f32,
    relief_strength: f32,
    fog_color: vec4<f32>,
    horizon_color: vec4<f32>,
    fog_range: vec4<f32>,
    fog_opacity: vec4<f32>,
    surface_color: vec4<f32>,
    fog_position: vec4<f32>,
    edge_heights: array<vec4<f32>, 128>,
    edge_last: vec4<f32>,
};

struct VertexOutput {
    @location(0) tex_coords: vec2<f32>,
    @location(1) horizon_distance: f32,
    // Distance along the view axis, the clip w, from which the fragment takes its fog.
    @location(2) eye_depth: f32,
    @location(3) surface_normal: vec3<f32>,
    @location(4) camera_relative_position: vec3<f32>,
    @builtin(position) clip_position: vec4<f32>,
};

@group(1) @binding(0) var<uniform> terrain_tile: TerrainTileUniforms;
@group(1) @binding(1) var dem_texture: texture_2d<f32>;

const TERRAIN_EXTENT: f32 = 4096.0;

// Decodes one DEM sample; the border texels stand in beyond the tile edges.
fn dem_sample(texel: vec2<i32>) -> f32 {
    let last = vec2<i32>(textureDimensions(dem_texture)) - vec2<i32>(1, 1);
    let rgb = textureLoad(dem_texture, clamp(texel, vec2<i32>(0, 0), last), 0).rgb * 255.0;
    return dot(rgb, terrain_tile.dem_unpack.xyz) - terrain_tile.dem_unpack.w;
}

// Bilinear elevation in metres at a tile position, read from the DEM tile this tile falls in.
fn terrain_height_gradient(position: vec2<f32>) -> vec3<f32> {
    let coord = (terrain_tile.dem_matrix * vec4<f32>(position, 0.0, 1.0)).xy * terrain_tile.dem_dim + 1.0;
    let f = fract(coord);
    let corner = vec2<i32>(floor(coord));
    let a = dem_sample(corner);
    let b = dem_sample(corner + vec2<i32>(1, 0));
    let c = dem_sample(corner + vec2<i32>(0, 1));
    let d = dem_sample(corner + vec2<i32>(1, 1));
    let units = terrain_tile.dem_matrix[0][0] * terrain_tile.dem_dim;
    return vec3<f32>(mix(mix(a,b,f.x), mix(c,d,f.x), f.y),
        mix(b-a,d-c,f.y)*units, mix(c-a,d-b,f.x)*units) * terrain_tile.exaggeration;
}
fn terrain_elevation(position: vec2<f32>) -> f32 { return terrain_height_gradient(position).x; }

// Edge samples follow the coarsest touching mesh, so refinement never opens a crack.
fn edge_height(t: f32, edge: u32) -> f32 {
    let i = u32(clamp(round(t / 32.0), 0.0, 128.0));
    if i == 128u { return terrain_tile.edge_last[edge] * terrain_tile.exaggeration; }
    return terrain_tile.edge_heights[i][edge] * terrain_tile.exaggeration;
}

fn stitched_elevation(position: vec2<f32>, height: f32) -> f32 {
    let p = clamp(position, vec2<f32>(0.0), vec2<f32>(TERRAIN_EXTENT));
    let w = 1.0 - smoothstep(vec4<f32>(0.0), vec4<f32>(64.0),
        vec4<f32>(p.y, TERRAIN_EXTENT - p.y, p.x, TERRAIN_EXTENT - p.x));
    if all(w == vec4<f32>(0.0)) { return height; }
    let delta = vec4<f32>(
        edge_height(p.x, 0u) - terrain_elevation(vec2<f32>(p.x, 0.0)),
        edge_height(p.x, 1u) - terrain_elevation(vec2<f32>(p.x, TERRAIN_EXTENT)),
        edge_height(p.y, 2u) - terrain_elevation(vec2<f32>(0.0, p.y)),
        edge_height(p.y, 3u) - terrain_elevation(vec2<f32>(TERRAIN_EXTENT, p.y)));
    let corner = vec4<f32>(
        edge_height(0.0, 0u) - terrain_elevation(vec2<f32>(0.0, 0.0)),
        edge_height(TERRAIN_EXTENT, 0u) - terrain_elevation(vec2<f32>(TERRAIN_EXTENT, 0.0)),
        edge_height(0.0, 1u) - terrain_elevation(vec2<f32>(0.0, TERRAIN_EXTENT)),
        edge_height(TERRAIN_EXTENT, 1u) - terrain_elevation(vec2<f32>(TERRAIN_EXTENT)));
    return height + dot(w, delta) - dot(corner, vec4<f32>(w.x*w.z, w.x*w.w, w.y*w.z, w.y*w.w));
}

@vertex
fn main(
    @location(0) raw_position: vec2<i32>,
    @location(1) skirt: vec2<u32>,
) -> VertexOutput {
    let position = clamp(vec2<f32>(raw_position), vec2<f32>(0.0), vec2<f32>(TERRAIN_EXTENT));
    var surface_position_2d = position;
    let north_cap = raw_position.y == -32768 && terrain_tile.tile_mercator_coords.y == 0.0;
    let south_cap = raw_position.y == 32767 &&
        terrain_tile.tile_mercator_coords.y + TERRAIN_EXTENT * terrain_tile.tile_mercator_coords.w >= 1.0;
    if north_cap || south_cap { surface_position_2d.y = f32(raw_position.y); }
    // Every longitude meets at the same altitude; unknown polar DEM samples cannot split the fan.
    let height_gradient = terrain_height_gradient(position);
    let elevation = select(stitched_elevation(position, height_gradient.x), 0.0, north_cap || south_cap)
        - f32(skirt.x) * terrain_tile.skirt_length;
    let projected = project_tile_position_3d(
        vec3<f32>(surface_position_2d, elevation),
        terrain_tile.transform,
        terrain_tile.tile_mercator_coords,
    );
    // The fog follows from the eye depth per fragment: it is not linear in depth, and the
    // triangles of a far tile span hundreds of kilometres, so a value found per vertex
    // would step at every tile edge.
    // Tile-relative metres keep slope lighting independent of zoom and eye orientation.
    let metres_per_unit = PROJECTION_TWO_PI * projection.transition_and_padding.z
        * terrain_tile.tile_mercator_coords.z
        * globe_circumference_ratio_at_tile_y(TERRAIN_EXTENT * 0.5, terrain_tile.tile_mercator_coords);
    var normal = normalize(vec3<f32>(-height_gradient.y, height_gradient.z, metres_per_unit));
    if north_cap || south_cap { normal = vec3<f32>(0.0, 0.0, 1.0); }
    return VertexOutput(
        position / TERRAIN_EXTENT,
        projected.horizon_distance,
        projected.clip_position.w,
        normal,
        terrain_tile.fog_position.xyz + vec3<f32>(position.x * terrain_tile.fog_position.w,
            -position.y * terrain_tile.fog_position.w, elevation),
        projected.clip_position,
    );
}
