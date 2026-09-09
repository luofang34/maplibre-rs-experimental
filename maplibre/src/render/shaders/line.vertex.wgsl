// @include projection.vertex.wgsl

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) v_color: vec4<f32>,
    @location(1) v_normal: vec2<f32>,
    @location(2) v_width2: vec2<f32>,
    @location(3) v_gamma_scale: f32,
    @location(4) horizon_distance: f32,
    @location(5) tile_x: f32,
    @location(6) @interpolate(flat) clip_antimeridian: u32,
    @location(7) dash: vec2<f32>,
};

@vertex
fn main(
    @location(0) position: vec2<f32>,
    @location(1) path: vec4<f32>,
    @location(2) tile_mercator_coords: vec4<f32>,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
    @location(8) color: vec4<f32>,
    @location(9) line_scale: vec2<f32>,
    @location(10) z_index: f32,
    @location(11) viewport_width: f32,
    @location(12) viewport_height: f32,
    @location(13) line_width: f32,
    @location(14) clip_antimeridian: u32,
    @location(15) layer_translate: vec2<f32>,
) -> VertexOutput {
    let normal = path.xy;
    let line_width_px = line_width * line_scale.x;
    let blur = 0.0;
    let gapwidth = 0.0;

    let halfwidth = line_width_px * 0.5;
    let pixel_ratio = 1.0;
    let antialiasing = (1.0 / pixel_ratio) * 0.5;

    let inset = gapwidth + select(0.0, antialiasing, gapwidth > 0.0);
    let outset = gapwidth + halfwidth * select(1.0, 2.0, gapwidth > 0.0) +
                 select(0.0, antialiasing, halfwidth != 0.0);

    let transform = mat4x4<f32>(translate1, translate2, translate3, translate4);

    let spatial = path.w > -1e20;
    let elevation = select(0.0, path.w, spatial);
    let projected_center = project_tile_position_3d(
        vec3<f32>(position + layer_translate, elevation),
        transform,
        tile_mercator_coords,
    );
    let projected_normal = project_tile_position_3d(
        vec3<f32>(position + layer_translate + normal, elevation),
        transform,
        tile_mercator_coords,
    );
    var center = projected_center.clip_position;
    let center_ndc = center.xy / center.w;
    let normal_ndc = projected_normal.clip_position.xy / projected_normal.clip_position.w;
    let direction = (normal_ndc - center_ndc) * vec2<f32>(viewport_width, viewport_height);
    let dir = direction / max(length(direction), 1e-12);

    // Apply pixel-width offset in clip space.
    // NDC spans 2 units across the viewport, so 1 pixel = 2/viewport_px in NDC.
    // Multiply by center.w to compensate for the perspective divide.
    // Use per-axis conversion to handle non-square viewports correctly.
    let px_to_clip_x = (2.0 / viewport_width) * center.w;
    let px_to_clip_y = (2.0 / viewport_height) * center.w;
    let clip_offset = vec2<f32>(dir.x * outset * px_to_clip_x, dir.y * outset * px_to_clip_y);
    if spatial {
        // Decks share the map-space width of adjoining draped roads, including foreshortening.
        let edge = project_tile_position_3d(
            vec3<f32>(position + layer_translate + normal * outset * line_scale.y, elevation),
            transform, tile_mercator_coords,
        );
        center = edge.clip_position;
        center.z += max(abs(center.z) * 2e-5, 1e-10);
    } else {
        center = vec4<f32>(center.xy + clip_offset, 0.0, center.w);
    }

    return VertexOutput(
        center,
        color,
        normal,
        vec2<f32>(outset, inset),
        1.0,
        projected_center.horizon_distance,
        position.x,
        clip_antimeridian,
        vec2<f32>(path.z / max(line_scale.y * line_width_px, 1e-6), line_width_px),
    );
}
