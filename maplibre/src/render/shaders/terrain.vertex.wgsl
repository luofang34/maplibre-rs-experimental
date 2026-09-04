// @include projection.vertex.wgsl

struct TerrainTileUniforms {
    transform: mat4x4<f32>,
    dem_matrix: mat4x4<f32>,
    tile_mercator_coords: vec4<f32>,
    dem_unpack: vec4<f32>,
    dem_dim: f32,
    exaggeration: f32,
    skirt_length: f32,
    padding: f32,
};

struct VertexOutput {
    @location(0) tex_coords: vec2<f32>,
    @location(1) horizon_distance: f32,
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
fn terrain_elevation(position: vec2<f32>) -> f32 {
    let coord = (terrain_tile.dem_matrix * vec4<f32>(position, 0.0, 1.0)).xy * terrain_tile.dem_dim + 1.0;
    let fraction = fract(coord);
    let corner = vec2<i32>(floor(coord));
    let top = mix(dem_sample(corner), dem_sample(corner + vec2<i32>(1, 0)), fraction.x);
    let bottom = mix(
        dem_sample(corner + vec2<i32>(0, 1)),
        dem_sample(corner + vec2<i32>(1, 1)),
        fraction.x,
    );
    return mix(top, bottom, fraction.y) * terrain_tile.exaggeration;
}

@vertex
fn main(
    @location(0) raw_position: vec2<i32>,
    @location(1) skirt: vec2<u32>,
) -> VertexOutput {
    let position = vec2<f32>(raw_position);
    let elevation = terrain_elevation(position) - f32(skirt.x) * terrain_tile.skirt_length;
    let projected = project_tile_position_3d(
        vec3<f32>(position, elevation),
        terrain_tile.transform,
        terrain_tile.tile_mercator_coords,
    );
    return VertexOutput(
        position / TERRAIN_EXTENT,
        projected.horizon_distance,
        projected.clip_position,
    );
}
