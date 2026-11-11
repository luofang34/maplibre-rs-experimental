// @include projection.vertex.wgsl

struct VertexOutput {
    @location(0) v_color: vec4<f32>,
    @location(4) horizon_distance: f32,
    // For a pattern: the position in the image space before the pattern size is applied, the
    // corner of the tile in the same units, and the light that shades the image.
    @location(5) pattern_pixels: vec2<f32>,
    @location(6) @interpolate(flat) origin_upper: vec2<f32>,
    @location(8) @interpolate(flat) origin_lower: vec2<f32>,
    @location(7) lighting: vec4<f32>,
    @builtin(position) @invariant position: vec4<f32>,
};

// The range of the packed rise, centred on zero, which one overscale step adds;
// `NEXT_ZOOM_CODE_SPAN` on the CPU.
const NEXT_ZOOM_CODE_SPAN: f32 = 65536.0;
// Stride per `(base_datum, height_datum)` pair packed above `next_zoom_code`.
const DATUM_CODE_SPAN: f32 = 1048576.0;

fn shade_extrusion(
    position: vec2<f32>,
    wall: vec2<f32>,
    tile_mercator_coords: vec4<f32>,
    base_in: f32,
    translate1: vec4<f32>,
    translate2: vec4<f32>,
    translate3: vec4<f32>,
    translate4: vec4<f32>,
    color: vec4<f32>,
    zoom_factor: f32,
    edge_and_next: vec2<f32>,
    height: f32,
    light_color: vec4<f32>,
    paint: vec4<f32>,
    light_position: vec4<f32>,
    layer_translate: vec2<f32>,
    translucent_shell: f32,
) -> VertexOutput {
    let length_of_wall = length(wall);
    let is_wall = length_of_wall > 0.5;
    let is_floor = !is_wall && length_of_wall > 0.1;
    let top = select(select(1.0, 0.0, is_floor), select(0.0, 1.0, length_of_wall > 1.5), is_wall);
    let cap_normal = select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 0.0, -1.0), is_floor);
    let normal = select(cap_normal, vec3<f32>(wall / max(length_of_wall, 1e-6), 0.0), is_wall);
    let edge_distance = edge_and_next.x;
    // Strip the packed vertical-datum pair before decoding overscale and zoom rise.
    let datum_code = floor(edge_and_next.y / DATUM_CODE_SPAN);
    let zoom_code = edge_and_next.y - datum_code * DATUM_CODE_SPAN;
    let overscale = floor(zoom_code / NEXT_ZOOM_CODE_SPAN);
    let rise = zoom_code - overscale * NEXT_ZOOM_CODE_SPAN - 0.5 * NEXT_ZOOM_CODE_SPAN;
    let default_blend = clamp(-log2(zoom_factor) - overscale, 0.0, 1.0);
    let top_blend = select(default_blend, paint.w - 1.0, paint.w >= 1.0);
    let base_blend = select(default_blend, light_position.w - 1.0, light_position.w >= 1.0);
    let zoom_blend = select(base_blend, top_blend, top > 0.5);
    let is_translucent = translucent_shell > 0.0;
    let raw_z = select(base_in, height, top > 0.5) + rise * zoom_blend;
    let z = select(raw_z, max(raw_z, base_in), is_translucent && top > 0.5);
    let roof = select(height, z, top > 0.5);
    let base = select(z, base_in, top > 0.5);
    let projected = project_tile_position_3d(
        vec3<f32>(position + layer_translate, z),
        mat4x4<f32>(translate1, translate2, translate3, translate4),
        tile_mercator_coords,
    );

    let intensity = paint.z;
    // Standard opaque extrusions darken RGB by `color.a`; translucent volumes keep vivid chromaticity
    // and carry `color.a` into true premultiplied alpha.
    let surface = select(color.rgb * color.a, color.rgb, is_translucent);
    let colorvalue = dot(surface, vec3<f32>(0.2126, 0.7152, 0.0722));
    let lit = surface + vec3<f32>(0.03);
    let graded = is_wall && trunc(normal.y * 16384.0) != 0.0;
    var directional = clamp(dot(normal, light_position.xyz), 0.0, 1.0);
    directional = mix(1.0 - intensity, max(1.0 - colorvalue + intensity, 1.0), directional);
    if graded {
        let gradient = clamp(
            (top + base) * pow(roof / 150.0, 0.5),
            mix(0.7, 0.98, 1.0 - intensity),
            1.0,
        );
        directional *= (1.0 - paint.y) + paint.y * gradient;
    }
    let rgb = clamp(
        lit * directional * light_color.rgb,
        mix(vec3<f32>(0.0), vec3<f32>(0.3), vec3<f32>(1.0) - light_color.rgb),
        vec3<f32>(1.0),
    );

    let tile_units_to_pixels = 1.0 / (8.0 * zoom_factor);
    let tile_scale = 1.0 / (tile_mercator_coords.z * 4096.0);
    let height_factor = -0.5 * tile_scale / 4096.0;
    let pattern_place = select(position, vec2<f32>(edge_distance, z * height_factor), is_wall);
    let view_scale = tile_scale / zoom_factor;
    let corner = floor(tile_mercator_coords.xy * tile_scale + vec2<f32>(0.5)) * 512.0 * (view_scale / tile_scale);
    let origin_upper = floor(corner / 65536.0);
    let origin_lower = corner - origin_upper * 65536.0;
    let pattern_light = mix(1.0 - intensity, max(0.5 + intensity, 1.0), clamp(dot(normal, light_position.xyz), 0.0, 1.0));
    var pattern_directional = pattern_light;
    if graded {
        let gradient = clamp(
            (top + base) * pow(roof / 150.0, 0.5),
            mix(0.7, 0.98, 1.0 - intensity),
            1.0,
        );
        pattern_directional *= (1.0 - paint.y) + paint.y * gradient;
    }
    let pattern_rgb = clamp(
        pattern_directional * light_color.rgb,
        mix(vec3<f32>(0.0), vec3<f32>(0.3), vec3<f32>(1.0) - light_color.rgb),
        vec3<f32>(1.0),
    );
    let alpha = select(
        paint.x,
        clamp(color.a * paint.x * translucent_shell, 0.0, 1.0),
        is_translucent,
    );
    return VertexOutput(
        vec4<f32>(rgb * alpha, alpha),
        projected.horizon_distance,
        pattern_place * tile_units_to_pixels,
        origin_upper,
        origin_lower,
        vec4<f32>(pattern_rgb * alpha, alpha),
        projected.clip_position,
    );
}

@vertex
fn main(
    @location(0) position: vec2<f32>,
    @location(1) wall: vec2<f32>,
    @location(2) tile_mercator_coords: vec4<f32>,
    @location(3) base_in: f32,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
    @location(8) color: vec4<f32>,
    @location(9) zoom_factor: f32,
    @location(10) edge_and_next: vec2<f32>,
    @location(11) height: f32,
    @location(12) light_color: vec4<f32>,
    @location(13) paint: vec4<f32>,
    @location(14) light_position: vec4<f32>,
    @location(15) layer_translate: vec2<f32>,
) -> VertexOutput {
    return shade_extrusion(
        position, wall, tile_mercator_coords, base_in,
        translate1, translate2, translate3, translate4,
        color, zoom_factor, edge_and_next, height,
        light_color, paint, light_position, layer_translate,
        0.0,
    );
}

@vertex
fn translucent_back_main(
    @location(0) position: vec2<f32>,
    @location(1) wall: vec2<f32>,
    @location(2) tile_mercator_coords: vec4<f32>,
    @location(3) base_in: f32,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
    @location(8) color: vec4<f32>,
    @location(9) zoom_factor: f32,
    @location(10) edge_and_next: vec2<f32>,
    @location(11) height: f32,
    @location(12) light_color: vec4<f32>,
    @location(13) paint: vec4<f32>,
    @location(14) light_position: vec4<f32>,
    @location(15) layer_translate: vec2<f32>,
) -> VertexOutput {
    return shade_extrusion(
        position, wall, tile_mercator_coords, base_in,
        translate1, translate2, translate3, translate4,
        color, zoom_factor, edge_and_next, height,
        light_color, paint, light_position, layer_translate,
        0.55,
    );
}

@vertex
fn translucent_front_main(
    @location(0) position: vec2<f32>,
    @location(1) wall: vec2<f32>,
    @location(2) tile_mercator_coords: vec4<f32>,
    @location(3) base_in: f32,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
    @location(8) color: vec4<f32>,
    @location(9) zoom_factor: f32,
    @location(10) edge_and_next: vec2<f32>,
    @location(11) height: f32,
    @location(12) light_color: vec4<f32>,
    @location(13) paint: vec4<f32>,
    @location(14) light_position: vec4<f32>,
    @location(15) layer_translate: vec2<f32>,
) -> VertexOutput {
    return shade_extrusion(
        position, wall, tile_mercator_coords, base_in,
        translate1, translate2, translate3, translate4,
        color, zoom_factor, edge_and_next, height,
        light_color, paint, light_position, layer_translate,
        1.0,
    );
}
