// @include projection.vertex.wgsl

struct DepthVertexOutput {
    @location(4) horizon_distance: f32,
    @builtin(position) @invariant position: vec4<f32>,
};

struct FlatDepthVertexOutput {
    @builtin(position) @invariant position: vec4<f32>,
};

struct ColorVertexOutput {
    @location(0) v_color: vec4<f32>,
    @location(4) horizon_distance: f32,
    @builtin(position) @invariant position: vec4<f32>,
};

struct FlatColorVertexOutput {
    @location(0) v_color: vec4<f32>,
    @builtin(position) @invariant position: vec4<f32>,
};

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

struct ExtrusionElevation {
    is_wall: bool,
    top: f32,
    z: f32,
    roof: f32,
    base: f32,
};

fn compute_extrusion_elevation(
    kind: u32,
    overscale: f32,
    base_in: f32,
    height: f32,
    rise: f32,
    zoom_factor: f32,
    paint: vec4<f32>,
    light_position: vec4<f32>,
    translucent_shell: f32,
) -> ExtrusionElevation {
    let is_wall = kind >= 2u;
    let is_floor = kind == 1u;
    let top = select(select(1.0, 0.0, is_floor), select(0.0, 1.0, kind == 3u), is_wall);
    let default_blend = clamp(-log2(zoom_factor) - overscale, 0.0, 1.0);
    let top_blend = select(default_blend, paint.w - 1.0, paint.w >= 1.0);
    let base_blend = select(default_blend, light_position.w - 1.0, light_position.w >= 1.0);
    let zoom_blend = select(base_blend, top_blend, top > 0.5);
    let is_translucent = translucent_shell > 0.0;
    let raw_z = select(base_in, height, top > 0.5) + rise * zoom_blend;
    let z = select(raw_z, max(raw_z, base_in), is_translucent && top > 0.5);
    let roof = select(height, z, top > 0.5);
    let base = select(z, base_in, top > 0.5);
    return ExtrusionElevation(is_wall, top, z, roof, base);
}

fn shade_extrusion_color(
    elev: ExtrusionElevation,
    normal: vec3<f32>,
    color: vec4<f32>,
    light_color: vec4<f32>,
    paint: vec4<f32>,
    light_position: vec4<f32>,
    translucent_shell: f32,
) -> vec4<f32> {
    let is_translucent = translucent_shell > 0.0;
    let intensity = paint.z;
    // Standard opaque extrusions darken RGB by `color.a`; translucent volumes keep vivid chromaticity
    // and carry `color.a` into true premultiplied alpha.
    let surface = select(color.rgb * color.a, color.rgb, is_translucent);
    let colorvalue = dot(surface, vec3<f32>(0.2126, 0.7152, 0.0722));
    let lit = surface + vec3<f32>(0.03);
    let graded = elev.is_wall && trunc(normal.y * 16384.0) != 0.0;
    var directional = clamp(dot(normal, light_position.xyz), 0.0, 1.0);
    directional = mix(1.0 - intensity, max(1.0 - colorvalue + intensity, 1.0), directional);
    if graded {
        let gradient = clamp(
            (elev.top + elev.base) * pow(elev.roof / 150.0, 0.5),
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
    let alpha = select(
        paint.x,
        clamp(color.a * paint.x * translucent_shell, 0.0, 1.0),
        is_translucent,
    );
    return vec4<f32>(rgb * alpha, alpha);
}

fn shade_unpacked_extrusion(
    position: vec2<f32>,
    kind: u32,
    normal: vec3<f32>,
    overscale: f32,
    edge_distance: f32,
    base_in: f32,
    height: f32,
    rise: f32,
    tile_mercator_coords: vec4<f32>,
    translate1: vec4<f32>,
    translate2: vec4<f32>,
    translate3: vec4<f32>,
    translate4: vec4<f32>,
    color: vec4<f32>,
    zoom_factor: f32,
    light_color: vec4<f32>,
    paint: vec4<f32>,
    light_position: vec4<f32>,
    layer_translate: vec2<f32>,
    translucent_shell: f32,
) -> VertexOutput {
    let elev = compute_extrusion_elevation(
        kind, overscale, base_in, height, rise, zoom_factor, paint, light_position, translucent_shell,
    );
    let projected = project_tile_position_3d(
        vec3<f32>(position + layer_translate, elev.z),
        mat4x4<f32>(translate1, translate2, translate3, translate4),
        tile_mercator_coords,
    );
    let v_color = shade_extrusion_color(
        elev, normal, color, light_color, paint, light_position, translucent_shell,
    );

    let intensity = paint.z;
    let graded = elev.is_wall && trunc(normal.y * 16384.0) != 0.0;
    let tile_units_to_pixels = 1.0 / (8.0 * zoom_factor);
    let tile_scale = 1.0 / (tile_mercator_coords.z * 4096.0);
    let height_factor = -0.5 * tile_scale / 4096.0;
    let pattern_place = select(position, vec2<f32>(edge_distance, elev.z * height_factor), elev.is_wall);
    let view_scale = tile_scale / zoom_factor;
    let corner = floor(tile_mercator_coords.xy * tile_scale + vec2<f32>(0.5)) * 512.0 * (view_scale / tile_scale);
    let origin_upper = floor(corner / 65536.0);
    let origin_lower = corner - origin_upper * 65536.0;
    let pattern_light = mix(1.0 - intensity, max(0.5 + intensity, 1.0), clamp(dot(normal, light_position.xyz), 0.0, 1.0));
    var pattern_directional = pattern_light;
    if graded {
        let gradient = clamp(
            (elev.top + elev.base) * pow(elev.roof / 150.0, 0.5),
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
    return VertexOutput(
        v_color,
        projected.horizon_distance,
        pattern_place * tile_units_to_pixels,
        origin_upper,
        origin_lower,
        vec4<f32>(pattern_rgb * v_color.a, v_color.a),
        projected.clip_position,
    );
}

fn shade_extrusion(
    v_pos_q2: vec2<i32>,
    v_wall_edge: u32,
    tile_mercator_coords: vec4<f32>,
    translate1: vec4<f32>,
    translate2: vec4<f32>,
    translate3: vec4<f32>,
    translate4: vec4<f32>,
    color: vec4<f32>,
    zoom_factor: f32,
    v_heights: vec2<u32>,
    light_color: vec4<f32>,
    paint: vec4<f32>,
    light_position: vec4<f32>,
    layer_translate: vec2<f32>,
    translucent_shell: f32,
) -> ColorVertexOutput {
    let position = vec2<f32>(v_pos_q2) * 0.25 + vec2<f32>(2048.0);
    let kind = v_wall_edge & 3u;
    let is_wall = kind >= 2u;
    let is_floor = kind == 1u;
    let angle = f32((v_wall_edge >> 2u) & 0x3fffu) * (PROJECTION_TWO_PI / 16384.0);
    let cap_normal = select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 0.0, -1.0), is_floor);
    let normal = select(cap_normal, vec3<f32>(cos(angle), sin(angle), 0.0), is_wall);
    let overscale = f32((v_wall_edge >> 16u) & 7u);
    let base_in = f32(bitcast<i32>(v_heights.x << 8u) >> 8u) / 16.0;
    let height = f32(bitcast<i32>(v_heights.y << 8u) >> 8u) / 16.0;
    let rise_bits = (v_heights.x >> 24u) | ((v_heights.y >> 16u) & 0xff00u);
    let rise = unpack2x16float(rise_bits).x;
    let elev = compute_extrusion_elevation(
        kind, overscale, base_in, height, rise, zoom_factor, paint, light_position, translucent_shell,
    );
    let projected = project_tile_position_3d(
        vec3<f32>(position + layer_translate, elev.z),
        mat4x4<f32>(translate1, translate2, translate3, translate4),
        tile_mercator_coords,
    );
    let v_color = shade_extrusion_color(
        elev, normal, color, light_color, paint, light_position, translucent_shell,
    );
    return ColorVertexOutput(v_color, projected.horizon_distance, projected.clip_position);
}

fn shade_compact_extrusion(
    v_pos_q2: vec2<i32>,
    v_packed: u32,
    tile_mercator_coords: vec4<f32>,
    translate1: vec4<f32>,
    translate2: vec4<f32>,
    translate3: vec4<f32>,
    translate4: vec4<f32>,
    color: vec4<f32>,
    zoom_factor: f32,
    light_color: vec4<f32>,
    paint: vec4<f32>,
    light_position: vec4<f32>,
    layer_translate: vec2<f32>,
    translucent_shell: f32,
) -> ColorVertexOutput {
    let position = vec2<f32>(v_pos_q2) * 0.25 + vec2<f32>(2048.0);
    let kind = v_packed & 3u;
    let is_wall = kind >= 2u;
    let is_floor = kind == 1u;
    let angle = f32((v_packed >> 2u) & 0x1ffu) * (PROJECTION_TWO_PI / 512.0);
    let cap_normal = select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 0.0, -1.0), is_floor);
    let normal = select(cap_normal, vec3<f32>(cos(angle), sin(angle), 0.0), is_wall);
    let base_in = f32((v_packed >> 11u) & 0x3ffu) * 0.5;
    let height = f32(v_packed >> 21u) * 0.5;
    let elev = compute_extrusion_elevation(
        kind, 0.0, base_in, height, 0.0, zoom_factor, paint, light_position, translucent_shell,
    );
    let projected = project_tile_position_3d(
        vec3<f32>(position + layer_translate, elev.z),
        mat4x4<f32>(translate1, translate2, translate3, translate4),
        tile_mercator_coords,
    );
    let v_color = shade_extrusion_color(
        elev, normal, color, light_color, paint, light_position, translucent_shell,
    );
    return ColorVertexOutput(v_color, projected.horizon_distance, projected.clip_position);
}

@vertex
fn depth_main(
    @location(0) v_pos_q2: vec2<i32>,
    @location(1) v_wall_edge: u32,
    @location(2) tile_mercator_coords: vec4<f32>,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
    @location(9) zoom_factor: f32,
    @location(10) v_heights: vec2<u32>,
    @location(13) paint: vec4<f32>,
    @location(14) light_position: vec4<f32>,
    @location(15) layer_translate: vec2<f32>,
) -> DepthVertexOutput {
    let position = vec2<f32>(v_pos_q2) * 0.25 + vec2<f32>(2048.0);
    let kind = v_wall_edge & 3u;
    let overscale = f32((v_wall_edge >> 16u) & 7u);
    let base_in = f32(bitcast<i32>(v_heights.x << 8u) >> 8u) / 16.0;
    let height = f32(bitcast<i32>(v_heights.y << 8u) >> 8u) / 16.0;
    let rise_bits = (v_heights.x >> 24u) | ((v_heights.y >> 16u) & 0xff00u);
    let rise = unpack2x16float(rise_bits).x;
    let elev = compute_extrusion_elevation(
        kind, overscale, base_in, height, rise, zoom_factor, paint, light_position, 0.0,
    );
    let projected = project_tile_position_3d(
        vec3<f32>(position + layer_translate, elev.z),
        mat4x4<f32>(translate1, translate2, translate3, translate4),
        tile_mercator_coords,
    );
    return DepthVertexOutput(projected.horizon_distance, projected.clip_position);
}

@vertex
fn compact_depth_main(
    @location(0) v_pos_q2: vec2<i32>,
    @location(1) v_packed: u32,
    @location(2) tile_mercator_coords: vec4<f32>,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
    @location(9) zoom_factor: f32,
    @location(13) paint: vec4<f32>,
    @location(14) light_position: vec4<f32>,
    @location(15) layer_translate: vec2<f32>,
) -> DepthVertexOutput {
    let position = vec2<f32>(v_pos_q2) * 0.25 + vec2<f32>(2048.0);
    let kind = v_packed & 3u;
    let base_in = f32((v_packed >> 11u) & 0x3ffu) * 0.5;
    let height = f32(v_packed >> 21u) * 0.5;
    let elev = compute_extrusion_elevation(
        kind, 0.0, base_in, height, 0.0, zoom_factor, paint, light_position, 0.0,
    );
    let projected = project_tile_position_3d(
        vec3<f32>(position + layer_translate, elev.z),
        mat4x4<f32>(translate1, translate2, translate3, translate4),
        tile_mercator_coords,
    );
    return DepthVertexOutput(projected.horizon_distance, projected.clip_position);
}

@vertex
fn pattern_main(
    @location(0) v_pos_q2: vec2<i32>,
    @location(1) v_wall_edge: u32,
    @location(2) tile_mercator_coords: vec4<f32>,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
    @location(8) color: vec4<f32>,
    @location(9) zoom_factor: f32,
    @location(10) v_heights: vec2<u32>,
    @location(12) light_color: vec4<f32>,
    @location(13) paint: vec4<f32>,
    @location(14) light_position: vec4<f32>,
    @location(15) layer_translate: vec2<f32>,
) -> VertexOutput {
    let position = vec2<f32>(v_pos_q2) * 0.25 + vec2<f32>(2048.0);
    let kind = v_wall_edge & 3u;
    let is_wall = kind >= 2u;
    let is_floor = kind == 1u;
    let angle = f32((v_wall_edge >> 2u) & 0x3fffu) * (PROJECTION_TWO_PI / 16384.0);
    let cap_normal = select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 0.0, -1.0), is_floor);
    let normal = select(cap_normal, vec3<f32>(cos(angle), sin(angle), 0.0), is_wall);
    let overscale = f32((v_wall_edge >> 16u) & 7u);
    let edge_distance = f32(v_wall_edge >> 19u) * 2.0;
    let base_in = f32(bitcast<i32>(v_heights.x << 8u) >> 8u) / 16.0;
    let height = f32(bitcast<i32>(v_heights.y << 8u) >> 8u) / 16.0;
    let rise_bits = (v_heights.x >> 24u) | ((v_heights.y >> 16u) & 0xff00u);
    let rise = unpack2x16float(rise_bits).x;
    return shade_unpacked_extrusion(
        position, kind, normal, overscale, edge_distance, base_in, height, rise,
        tile_mercator_coords, translate1, translate2, translate3, translate4,
        color, zoom_factor, light_color, paint, light_position, layer_translate,
        0.0,
    );
}

@vertex
fn main(
    @location(0) v_pos_q2: vec2<i32>,
    @location(1) v_wall_edge: u32,
    @location(2) tile_mercator_coords: vec4<f32>,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
    @location(8) color: vec4<f32>,
    @location(9) zoom_factor: f32,
    @location(10) v_heights: vec2<u32>,
    @location(12) light_color: vec4<f32>,
    @location(13) paint: vec4<f32>,
    @location(14) light_position: vec4<f32>,
    @location(15) layer_translate: vec2<f32>,
) -> ColorVertexOutput {
    return shade_extrusion(
        v_pos_q2, v_wall_edge, tile_mercator_coords,
        translate1, translate2, translate3, translate4,
        color, zoom_factor, v_heights,
        light_color, paint, light_position, layer_translate,
        0.0,
    );
}

@vertex
fn translucent_back_main(
    @location(0) v_pos_q2: vec2<i32>,
    @location(1) v_wall_edge: u32,
    @location(2) tile_mercator_coords: vec4<f32>,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
    @location(8) color: vec4<f32>,
    @location(9) zoom_factor: f32,
    @location(10) v_heights: vec2<u32>,
    @location(12) light_color: vec4<f32>,
    @location(13) paint: vec4<f32>,
    @location(14) light_position: vec4<f32>,
    @location(15) layer_translate: vec2<f32>,
) -> ColorVertexOutput {
    return shade_extrusion(
        v_pos_q2, v_wall_edge, tile_mercator_coords,
        translate1, translate2, translate3, translate4,
        color, zoom_factor, v_heights,
        light_color, paint, light_position, layer_translate,
        0.55,
    );
}

@vertex
fn translucent_front_main(
    @location(0) v_pos_q2: vec2<i32>,
    @location(1) v_wall_edge: u32,
    @location(2) tile_mercator_coords: vec4<f32>,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
    @location(8) color: vec4<f32>,
    @location(9) zoom_factor: f32,
    @location(10) v_heights: vec2<u32>,
    @location(12) light_color: vec4<f32>,
    @location(13) paint: vec4<f32>,
    @location(14) light_position: vec4<f32>,
    @location(15) layer_translate: vec2<f32>,
) -> ColorVertexOutput {
    return shade_extrusion(
        v_pos_q2, v_wall_edge, tile_mercator_coords,
        translate1, translate2, translate3, translate4,
        color, zoom_factor, v_heights,
        light_color, paint, light_position, layer_translate,
        1.0,
    );
}

@vertex
fn compact_main(
    @location(0) v_pos_q2: vec2<i32>,
    @location(1) v_packed: u32,
    @location(2) tile_mercator_coords: vec4<f32>,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
    @location(8) color: vec4<f32>,
    @location(9) zoom_factor: f32,
    @location(12) light_color: vec4<f32>,
    @location(13) paint: vec4<f32>,
    @location(14) light_position: vec4<f32>,
    @location(15) layer_translate: vec2<f32>,
) -> ColorVertexOutput {
    return shade_compact_extrusion(
        v_pos_q2, v_packed, tile_mercator_coords,
        translate1, translate2, translate3, translate4,
        color, zoom_factor,
        light_color, paint, light_position, layer_translate,
        0.0,
    );
}

@vertex
fn compact_translucent_back_main(
    @location(0) v_pos_q2: vec2<i32>,
    @location(1) v_packed: u32,
    @location(2) tile_mercator_coords: vec4<f32>,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
    @location(8) color: vec4<f32>,
    @location(9) zoom_factor: f32,
    @location(12) light_color: vec4<f32>,
    @location(13) paint: vec4<f32>,
    @location(14) light_position: vec4<f32>,
    @location(15) layer_translate: vec2<f32>,
) -> ColorVertexOutput {
    return shade_compact_extrusion(
        v_pos_q2, v_packed, tile_mercator_coords,
        translate1, translate2, translate3, translate4,
        color, zoom_factor,
        light_color, paint, light_position, layer_translate,
        0.55,
    );
}

@vertex
fn compact_translucent_front_main(
    @location(0) v_pos_q2: vec2<i32>,
    @location(1) v_packed: u32,
    @location(2) tile_mercator_coords: vec4<f32>,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
    @location(8) color: vec4<f32>,
    @location(9) zoom_factor: f32,
    @location(12) light_color: vec4<f32>,
    @location(13) paint: vec4<f32>,
    @location(14) light_position: vec4<f32>,
    @location(15) layer_translate: vec2<f32>,
) -> ColorVertexOutput {
    return shade_compact_extrusion(
        v_pos_q2, v_packed, tile_mercator_coords,
        translate1, translate2, translate3, translate4,
        color, zoom_factor,
        light_color, paint, light_position, layer_translate,
        1.0,
    );
}

@vertex
fn flat_depth_main(
    @location(0) v_pos_q2: vec2<i32>,
    @location(1) v_wall_edge: u32,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
    @location(9) zoom_factor: f32,
    @location(10) v_heights: vec2<u32>,
    @location(13) paint: vec4<f32>,
    @location(14) light_position: vec4<f32>,
    @location(15) layer_translate: vec2<f32>,
) -> FlatDepthVertexOutput {
    let position = vec2<f32>(v_pos_q2) * 0.25 + vec2<f32>(2048.0);
    let kind = v_wall_edge & 3u;
    let overscale = f32((v_wall_edge >> 16u) & 7u);
    let base_in = f32(bitcast<i32>(v_heights.x << 8u) >> 8u) / 16.0;
    let height = f32(bitcast<i32>(v_heights.y << 8u) >> 8u) / 16.0;
    let rise_bits = (v_heights.x >> 24u) | ((v_heights.y >> 16u) & 0xff00u);
    let rise = unpack2x16float(rise_bits).x;
    let elev = compute_extrusion_elevation(
        kind, overscale, base_in, height, rise, zoom_factor, paint, light_position, 0.0,
    );
    let clip = mat4x4<f32>(translate1, translate2, translate3, translate4)
        * vec4<f32>(position + layer_translate, elev.z, 1.0);
    return FlatDepthVertexOutput(clip);
}

@vertex
fn flat_compact_depth_main(
    @location(0) v_pos_q2: vec2<i32>,
    @location(1) v_packed: u32,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
    @location(9) zoom_factor: f32,
    @location(13) paint: vec4<f32>,
    @location(14) light_position: vec4<f32>,
    @location(15) layer_translate: vec2<f32>,
) -> FlatDepthVertexOutput {
    let position = vec2<f32>(v_pos_q2) * 0.25 + vec2<f32>(2048.0);
    let kind = v_packed & 3u;
    let base_in = f32((v_packed >> 11u) & 0x3ffu) * 0.5;
    let height = f32(v_packed >> 21u) * 0.5;
    let elev = compute_extrusion_elevation(
        kind, 0.0, base_in, height, 0.0, zoom_factor, paint, light_position, 0.0,
    );
    let clip = mat4x4<f32>(translate1, translate2, translate3, translate4)
        * vec4<f32>(position + layer_translate, elev.z, 1.0);
    return FlatDepthVertexOutput(clip);
}

@vertex
fn flat_main(
    @location(0) v_pos_q2: vec2<i32>,
    @location(1) v_wall_edge: u32,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
    @location(8) color: vec4<f32>,
    @location(9) zoom_factor: f32,
    @location(10) v_heights: vec2<u32>,
    @location(12) light_color: vec4<f32>,
    @location(13) paint: vec4<f32>,
    @location(14) light_position: vec4<f32>,
    @location(15) layer_translate: vec2<f32>,
) -> FlatColorVertexOutput {
    let position = vec2<f32>(v_pos_q2) * 0.25 + vec2<f32>(2048.0);
    let kind = v_wall_edge & 3u;
    let is_wall = kind >= 2u;
    let is_floor = kind == 1u;
    let angle = f32((v_wall_edge >> 2u) & 0x3fffu) * (PROJECTION_TWO_PI / 16384.0);
    let cap_normal = select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 0.0, -1.0), is_floor);
    let normal = select(cap_normal, vec3<f32>(cos(angle), sin(angle), 0.0), is_wall);
    let overscale = f32((v_wall_edge >> 16u) & 7u);
    let base_in = f32(bitcast<i32>(v_heights.x << 8u) >> 8u) / 16.0;
    let height = f32(bitcast<i32>(v_heights.y << 8u) >> 8u) / 16.0;
    let rise_bits = (v_heights.x >> 24u) | ((v_heights.y >> 16u) & 0xff00u);
    let rise = unpack2x16float(rise_bits).x;
    let elev = compute_extrusion_elevation(
        kind, overscale, base_in, height, rise, zoom_factor, paint, light_position, 0.0,
    );
    let clip = mat4x4<f32>(translate1, translate2, translate3, translate4)
        * vec4<f32>(position + layer_translate, elev.z, 1.0);
    let v_color = shade_extrusion_color(
        elev, normal, color, light_color, paint, light_position, 0.0,
    );
    return FlatColorVertexOutput(v_color, clip);
}

@vertex
fn flat_compact_main(
    @location(0) v_pos_q2: vec2<i32>,
    @location(1) v_packed: u32,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
    @location(8) color: vec4<f32>,
    @location(9) zoom_factor: f32,
    @location(12) light_color: vec4<f32>,
    @location(13) paint: vec4<f32>,
    @location(14) light_position: vec4<f32>,
    @location(15) layer_translate: vec2<f32>,
) -> FlatColorVertexOutput {
    let position = vec2<f32>(v_pos_q2) * 0.25 + vec2<f32>(2048.0);
    let kind = v_packed & 3u;
    let is_wall = kind >= 2u;
    let is_floor = kind == 1u;
    let angle = f32((v_packed >> 2u) & 0x1ffu) * (PROJECTION_TWO_PI / 512.0);
    let cap_normal = select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 0.0, -1.0), is_floor);
    let normal = select(cap_normal, vec3<f32>(cos(angle), sin(angle), 0.0), is_wall);
    let base_in = f32((v_packed >> 11u) & 0x3ffu) * 0.5;
    let height = f32(v_packed >> 21u) * 0.5;
    let elev = compute_extrusion_elevation(
        kind, 0.0, base_in, height, 0.0, zoom_factor, paint, light_position, 0.0,
    );
    let clip = mat4x4<f32>(translate1, translate2, translate3, translate4)
        * vec4<f32>(position + layer_translate, elev.z, 1.0);
    let v_color = shade_extrusion_color(
        elev, normal, color, light_color, paint, light_position, 0.0,
    );
    return FlatColorVertexOutput(v_color, clip);
}
