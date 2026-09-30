// @include projection.vertex.wgsl

struct PatternUniforms {
    // Width and height of the pattern image in layout pixels.
    size: vec4<f32>,
};

@group(1) @binding(0) var<uniform> pattern: PatternUniforms;

struct VertexOutput {
    @location(0) v_color: vec4<f32>,
    @location(1) pattern_position: vec2<f32>,
    @location(4) horizon_distance: f32,
    @builtin(position) position: vec4<f32>,
};

@vertex
fn main(
    @location(0) position: vec2<f32>,
    @location(1) normal: vec2<f32>,
    @location(2) tile_mercator_coords: vec4<f32>,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
    @location(8) color: vec4<f32>,
    @location(9) zoom_factor: f32,
    @location(10) z_index: f32,
    @location(15) layer_translate: vec2<f32>,
) -> VertexOutput {
    let projected = project_tile_position(
        vec3<f32>(position + layer_translate, 0.0),
        mat4x4<f32>(translate1, translate2, translate3, translate4),
        tile_mercator_coords,
    );
    var final_position = projected.clip_position;
    final_position.z = 0.0;

    // The pattern is fixed to the pixel grid of the whole map at the view zoom, so tiles
    // agree where they meet: the tile's corner in map pixels, taken modulo the pattern size,
    // plus the position in the tile in pixels.
    let tile_units_to_pixels = 1.0 / (8.0 * zoom_factor);
    // The tile's column and row are exact integers and scaling them by powers of two is
    // exact, so the corner splits without loss into a high and a low part, as GL JS does,
    // and the modulo of the pattern size is taken in steps that stay small.
    let tile_scale = 1.0 / (tile_mercator_coords.z * 4096.0);
    let corner = floor(tile_mercator_coords.xy * tile_scale + vec2<f32>(0.5)) * 512.0 / zoom_factor;
    let upper = floor(corner / 65536.0);
    let lower = corner - upper * 65536.0;
    let size = pattern.size.xy;
    let upper_mod = upper - size * floor(upper / size);
    let coarse = upper_mod * 256.0 - size * floor(upper_mod * 256.0 / size);
    let low = coarse * 256.0 + lower;
    let offset = low - size * floor(low / size);
    let pattern_position = (position + layer_translate) * tile_units_to_pixels + offset;

    return VertexOutput(color, pattern_position, projected.horizon_distance, final_position);
}
