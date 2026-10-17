// @include line_ground.wgsl
// @include terrain_surface.wgsl.part

// A line over the terrain surface. Each draw binds one terrain tile, and the line lies on the
// mesh drawn for that tile; the fragments beyond it come from the draws of its neighbours.
@group(2) @binding(0) var<uniform> terrain_tile: TerrainTileUniforms;
@group(2) @binding(1) var dem_texture: texture_2d<f32>;

fn line_ground(position: vec2<f32>, tile_mercator_coords: vec4<f32>) -> LineGround {
    let terrain = terrain_tile.tile_mercator_coords;
    // Both origins are whole multiples of a power-of-two tile size, as is the ratio of the
    // scales, so the position in the terrain tile is exact in f32 at any zoom.
    let scale = tile_mercator_coords.zw / terrain.zw;
    let terrain_position = (tile_mercator_coords.xy - terrain.xy) / terrain.zw + position * scale;
    let surface = terrain_mesh_height_gradient(terrain_position);
    return LineGround(true, surface.x, surface.yz * scale, terrain_position);
}
