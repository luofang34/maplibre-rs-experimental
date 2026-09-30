// @include projection.vertex.wgsl

struct VertexOutput {
    @location(0) tex_coords: vec3<f32>,
    @location(1) horizon_distance: f32,
    @builtin(position) clip_position: vec4<f32>,
};

const EXTENT: f32 = 4096.0;

struct RasterPaint {
    spin_weights: vec4<f32>,
    opacity: f32,
    saturation_factor: f32,
    contrast_factor: f32,
    brightness_min: f32,
    brightness_max: f32,
    // Screen shift in clip units per unit of w that aligns the texels with the pixels.
    align: vec2<f32>,
};

@group(2) @binding(0)
var<uniform> paint: RasterPaint;

@vertex
fn main(
    @location(0) raw_position: vec2<i32>,
    @location(2) tile_mercator_coords: vec4<f32>,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
) -> VertexOutput {
    let tile_position = vec3<f32>(vec2<f32>(raw_position), 0.0);
    let projected = project_tile_mesh_position(
        tile_position,
        raw_position,
        mat4x4<f32>(translate1, translate2, translate3, translate4),
        tile_mercator_coords,
    );
    var tex_coords = vec2<f32>(raw_position) / EXTENT;
    if raw_position.y < -32767 {
        tex_coords.y = 0.0;
    } else if raw_position.y > 32766 {
        tex_coords.y = 1.0;
    }
    var clip = projected.clip_position;
    // The globe is not aligned: its clip positions are not those of a flat map.
    if projection.transition_and_padding.x < 0.5 {
        clip = vec4<f32>(clip.xy + paint.align * clip.w, clip.zw);
    }
    return VertexOutput(
        vec3<f32>(tex_coords, 1.0),
        projected.horizon_distance,
        clip,
    );
}
