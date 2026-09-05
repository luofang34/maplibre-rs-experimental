// @include projection.vertex.wgsl

struct VertexOutput {
    @location(1) v_data0: vec2<f32>,
    @location(2) v_data1: vec3<f32>,
    @location(4) horizon_distance: f32,
    @builtin(position) position: vec4<f32>,
};

// The terrain tile the symbols stand on; zeroed when the map has no terrain.
struct TerrainTileUniforms {
    transform: mat4x4<f32>,
    dem_matrix: mat4x4<f32>,
    tile_mercator_coords: vec4<f32>,
    dem_unpack: vec4<f32>,
    dem_dim: f32,
    exaggeration: f32,
    skirt_length: f32,
    padding: f32,
    fog_color: vec4<f32>,
    horizon_color: vec4<f32>,
    fog_range: vec4<f32>,
    fog_opacity: vec4<f32>,
};

@group(2) @binding(0) var<uniform> terrain_tile: TerrainTileUniforms;
@group(2) @binding(1) var dem_texture: texture_2d<f32>;

fn dem_sample(texel: vec2<i32>) -> f32 {
    let last = vec2<i32>(textureDimensions(dem_texture)) - vec2<i32>(1, 1);
    let rgb = textureLoad(dem_texture, clamp(texel, vec2<i32>(0, 0), last), 0).rgb * 255.0;
    return dot(rgb, terrain_tile.dem_unpack.xyz) - terrain_tile.dem_unpack.w;
}

// Bilinear elevation in metres at a tile position, as the terrain mesh samples it, so a symbol
// anchor lands on the drawn surface.
fn terrain_elevation(position: vec2<f32>) -> f32 {
    if terrain_tile.exaggeration == 0.0 {
        return 0.0;
    }
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
    @location(0) a_pos_offset: vec4<i32>,
    @location(1) a_data: vec4<u32>,
    @location(2) a_pixeloffset: vec4<i32>,
    @location(3) tile_mercator_coords: vec4<f32>,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
    @location(9) zoom_factor: f32,
    @location(10) z_index: f32,
    @location(12) opacity: f32,
    @location(13) text_size: f32,
    @builtin(instance_index) instance_idx: u32,
) -> VertexOutput {
    let anchor = vec2<f32>(a_pos_offset.xy);
    let glyph_offset = vec2<f32>(a_pos_offset.zw) / 32.0;
    let pixel_offset = vec2<f32>(a_pixeloffset.xy) / 16.0;
    let size = select(16.0, text_size, text_size > 0.0);
    let font_scale = size / 24.0;
    let latitude_scale = project_symbol_scale(anchor.y, tile_mercator_coords);
    let tile_position = anchor + (glyph_offset * font_scale + pixel_offset) * latitude_scale;
    let transform = mat4x4<f32>(translate1, translate2, translate3, translate4);
    let elevation = terrain_elevation(anchor);
    var projected = project_tile_position(
        vec3<f32>(tile_position, 0.0),
        transform,
        tile_mercator_coords,
    );
    if elevation != 0.0 {
        projected = project_tile_position_3d(
            vec3<f32>(tile_position, elevation),
            transform,
            tile_mercator_coords,
        );
    }
    var final_position = projected.clip_position;
    final_position.z = z_index;

    let tex_size = vec2<f32>(3178.0, 30.0);
    let texture_coordinates = vec2<f32>(a_data.xy) / tex_size;
    let gamma_scale = max(abs(final_position.w), 1e-6);
    return VertexOutput(
        texture_coordinates,
        vec3<f32>(gamma_scale, size, opacity),
        projected.horizon_distance,
        final_position,
    );
}
