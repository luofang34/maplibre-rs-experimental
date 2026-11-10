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
    // A blur that varies by feature, negative to leave the layer's own; the ground under the
    // circle, below -1e20 off terrain; the radius at the zoom above; and the corner code, whose
    // fraction carries a stroke opacity that varies by feature.
    @location(10) per_feature: vec4<f32>,
    @location(2) tile_mercator_coords: vec4<f32>,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
    @location(8) color: vec4<f32>,
    @location(9) zoom_factor: f32,
    @location(11) viewport_width: f32,
    @location(12) viewport_height: f32,
    @location(15) layer_translate: vec2<f32>,
    @location(3) stroke_color: vec4<f32>,
    @location(13) circle_params: vec4<f32>,
    @location(14) circle_flags: vec4<f32>,
) -> VertexOutput {
    // Quads are emitted with four consecutive vertices; the corner follows the vertex order.
    let corner = vertex_index % 4u;
    var extrude = vec2<f32>(
        select(-1.0, 1.0, corner == 1u || corner == 2u),
        select(-1.0, 1.0, corner >= 2u),
    );
    let corner_code = floor(per_feature.w);
    let stroke_opacity_code = per_feature.w - corner_code;
    // A subdivided circle names its corner on an eight-by-eight grid, to follow the globe.
    if corner_code > 0.5 {
        let cell = u32(corner_code) - 1u;
        extrude = vec2<f32>(f32(cell / 8u), f32(cell % 8u)) / 7.0 * 2.0 - vec2<f32>(1.0);
    }
    // The radius of a feature that varies with zoom is blended toward the zoom above the tile's.
    let zoom_blend = clamp(-log2(zoom_factor), 0.0, 1.0);
    let radius = mix(normal.x, per_feature.z, zoom_blend);
    let stroke_width = normal.y;
    let total = max(radius + stroke_width, 1e-3);
    let transform = mat4x4<f32>(translate1, translate2, translate3, translate4);
    let center = position + layer_translate;
    // Clip-space w of the view center, so pitch-scale `map` shrinks circles with distance.
    let center_distance = projection.transition_and_padding.y;
    let scale_with_map = circle_flags.x > 0.5;
    let pitch_with_map = circle_flags.y > 0.5;
    // Over terrain the circle stands on the ground under its centre, as GL JS draws it.
    let on_terrain = per_feature.y > -1e20;
    let height = select(0.0, per_feature.y, on_terrain);
    let projected_center = project_circle_center(center, height, on_terrain, transform, tile_mercator_coords);

    var clip: vec4<f32>;
    var horizon: f32;
    if pitch_with_map {
        var pixels = total;
        if !scale_with_map {
            // Lying on the map already scales with distance; undo it at the center.
            pixels = total * projected_center.clip_position.w / center_distance;
        }
        let corner_offset = extrude * pixels * TILE_UNITS_PER_PIXEL * zoom_factor;
        let corner_position = center + corner_offset;
        // On the globe the circle keeps its angular size, not the stretch of the Mercator
        // plane it was laid out in, so its corners are turned on the sphere from its center.
        let transition = projection.transition_and_padding.x;
        let mercator_clip = transform * vec4<f32>(corner_position, height, 1.0);
        let center_vector = tile_position_on_unit_sphere(center, tile_mercator_coords);
        let angles = corner_offset * tile_mercator_coords.z * PROJECTION_TWO_PI * projection.globe_circle.x;
        let lift = 1.0 + height / projection.transition_and_padding.z;
        let globe_clip = projection.main_matrix * vec4<f32>(globe_rotate_vector(center_vector, angles) * lift, 1.0);
        clip = interpolate_clip_position(mercator_clip, globe_clip, transition);
        horizon = projected_center.horizon_distance;
    } else {
        clip = projected_center.clip_position;
        horizon = projected_center.horizon_distance;
        let depth_scale = select(clip.w, center_distance, scale_with_map);
        let px_to_clip = vec2<f32>(2.0 / viewport_width, 2.0 / viewport_height);
        clip = vec4<f32>(clip.xy + extrude * total * px_to_clip * depth_scale, clip.z, clip.w);
    }
    clip.z = 0.0;

    // Roughly one pixel of blur keeps the edge antialiased whatever the radius.
    let blur = select(circle_params.z, per_feature.x, per_feature.x >= 0.0);
    let stroke_opacity = select(
        circle_params.y,
        (stroke_opacity_code - 0.25) * 2.0,
        stroke_opacity_code > 0.125,
    );
    let antialiasblur = -max(1.0 / total, blur);
    // A stroke colour that varies by feature rides packed with the fill in the feature colour.
    var fill = color;
    var stroke = stroke_color;
    if color.x < 0.0 {
        fill = vec4<f32>(unpack_byte_pair(color.x), unpack_byte_pair(color.y)) / 255.0;
        stroke = vec4<f32>(unpack_byte_pair(color.z), unpack_byte_pair(color.w)) / 255.0;
    }
    return VertexOutput(
        clip,
        fill,
        stroke,
        vec3<f32>(extrude, antialiasblur),
        vec4<f32>(radius / total, circle_params.x, stroke_opacity, stroke_width),
        horizon,
    );
}

// Two 8-bit channels packed into one float as `-(256 high + low) - 1`.
fn unpack_byte_pair(packed: f32) -> vec2<f32> {
    let pair = -packed - 1.0;
    let high = floor(pair / 256.0);
    return vec2<f32>(high, pair - high * 256.0);
}

// The centre of a circle, on the terrain at `height` metres when it stands there; off terrain
// it is projected exactly as on the map plane.
fn project_circle_center(
    center: vec2<f32>,
    height: f32,
    on_terrain: bool,
    transform: mat4x4<f32>,
    tile_mercator_coords: vec4<f32>,
) -> ProjectedTilePosition {
    if on_terrain {
        return project_tile_position_3d(vec3<f32>(center, height), transform, tile_mercator_coords);
    }
    return project_tile_position(vec3<f32>(center, 0.0), transform, tile_mercator_coords);
}
