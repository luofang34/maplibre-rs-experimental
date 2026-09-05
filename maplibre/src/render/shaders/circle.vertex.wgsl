// @include projection.vertex.wgsl

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) v_color: vec4<f32>,
    @location(1) v_stroke_color: vec4<f32>,
    // Extrusion direction and the negative antialiasing blur.
    @location(2) v_data: vec3<f32>,
    // Radius ratio, opacity, stroke opacity, stroke width.
    @location(3) v_params: vec4<f32>,
    @location(4) horizon_distance: f32,
};

// A 512-pixel view tile spans the 4096-unit tile grid.
const TILE_UNITS_PER_PIXEL: f32 = 8.0;

@vertex
fn main(
    @builtin(vertex_index) vertex_index: u32,
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
    @location(11) viewport_width: f32,
    @location(12) viewport_height: f32,
    @location(15) layer_translate: vec2<f32>,
    @location(16) stroke_color: vec4<f32>,
    @location(17) circle_params: vec4<f32>,
    @location(18) circle_flags: vec4<f32>,
) -> VertexOutput {
    // Quads are emitted with four consecutive vertices; the corner follows the vertex order.
    let corner = vertex_index % 4u;
    let extrude = vec2<f32>(
        select(-1.0, 1.0, corner == 1u || corner == 2u),
        select(-1.0, 1.0, corner >= 2u),
    );
    let radius = normal.x;
    let stroke_width = normal.y;
    let total = max(radius + stroke_width, 1e-3);
    let transform = mat4x4<f32>(translate1, translate2, translate3, translate4);
    let center = position + layer_translate;
    // Clip-space w of the view center, so pitch-scale `map` shrinks circles with distance.
    let center_distance = projection.transition_and_padding.y;
    let scale_with_map = circle_flags.x > 0.5;
    let pitch_with_map = circle_flags.y > 0.5;

    var clip: vec4<f32>;
    var horizon: f32;
    if pitch_with_map {
        let projected_center = project_tile_position(
            vec3<f32>(center, 0.0),
            transform,
            tile_mercator_coords,
        );
        var pixels = total;
        if !scale_with_map {
            // Lying on the map already scales with distance; undo it at the center.
            pixels = total * projected_center.clip_position.w / center_distance;
        }
        let corner_position = center + extrude * pixels * TILE_UNITS_PER_PIXEL * zoom_factor;
        let projected = project_tile_position(
            vec3<f32>(corner_position, 0.0),
            transform,
            tile_mercator_coords,
        );
        clip = projected.clip_position;
        horizon = projected.horizon_distance;
    } else {
        let projected = project_tile_position(
            vec3<f32>(center, 0.0),
            transform,
            tile_mercator_coords,
        );
        clip = projected.clip_position;
        horizon = projected.horizon_distance;
        let depth_scale = select(clip.w, center_distance, scale_with_map);
        let px_to_clip = vec2<f32>(2.0 / viewport_width, 2.0 / viewport_height);
        clip = vec4<f32>(clip.xy + extrude * total * px_to_clip * depth_scale, clip.z, clip.w);
    }
    clip.z = z_index;

    // Roughly one pixel of blur keeps the edge antialiased whatever the radius.
    let antialiasblur = -max(1.0 / total, circle_params.z);
    return VertexOutput(
        clip,
        color,
        stroke_color,
        vec3<f32>(extrude, antialiasblur),
        vec4<f32>(radius / total, circle_params.x, circle_params.y, stroke_width),
        horizon,
    );
}
