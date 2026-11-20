// @include projection.vertex.wgsl

struct PlainGlobeLineVertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) v_color: vec4<f32>,
    @location(1) v_normal: vec2<f32>,
    @location(2) @interpolate(flat) v_width2: vec2<f32>,
    @location(3) @interpolate(flat) blur: f32,
    @location(4) tile_x: f32,
    @location(5) @interpolate(flat) clip_antimeridian: u32,
};

@vertex
fn main(
    @location(0) v_pos_q2: vec2<i32>,
    @location(1) v_norm_packed: u32,
    @location(2) tile_mercator_coords: vec4<f32>,
    @location(3) line_style: vec4<f32>,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
    @location(8) color: vec4<f32>,
    @location(9) line_scale: vec2<f32>,
    @location(10) v_meta: vec2<u32>,
    @location(13) line_width: f32,
    @location(14) clip_antimeridian: u32,
    @location(15) layer_translate: vec2<f32>,
) -> PlainGlobeLineVertexOutput {
    let position = vec2<f32>(v_pos_q2) * 0.25 + vec2<f32>(2048.0);
    let normal = unpack2x16float(v_norm_packed);
    let w3 = v_meta.y;
    let negative_side = ((w3 >> 30u) & 1u) != 0u;
    let per_feature = ((w3 >> 29u) & 1u) != 0u;
    let feature_width = f32((w3 >> 18u) & 0x7ffu) / 16.0;
    let feature_offset = f32(i32((w3 >> 8u) & 0x3ffu) - 512) / 8.0;
    let line_width_px = select(line_width, feature_width, per_feature) * line_scale.x;
    let line_offset = select(line_style.x, feature_offset, per_feature);
    let gap_u8 = w3 & 0xffu;
    let blur_u8 = (v_meta.x >> 24u) & 0xffu;
    let gap_width = select(f32(gap_u8) / 8.0, line_style.y, gap_u8 == 0xffu);
    let blur_width = select(f32(blur_u8) / 8.0, line_style.z, blur_u8 == 0xffu);
    let gapwidth = gap_width * 0.5;

    let halfwidth = line_width_px * 0.5;
    let antialiasing = 0.5;
    let inset = gapwidth + select(0.0, antialiasing, gapwidth > 0.0);
    let outset = gapwidth + halfwidth * select(1.0, 2.0, gapwidth > 0.0)
        + select(0.0, antialiasing, halfwidth != 0.0);

    let transform = mat4x4<f32>(translate1, translate2, translate3, translate4);
    let pos_2d = position + layer_translate;
    let sp = tile_surface_point(pos_2d, tile_mercator_coords);
    let projected = project_tile_position_3d_from_surface(
        sp,
        vec3<f32>(pos_2d, 0.0),
        transform,
        tile_mercator_coords,
    );
    let tangent = project_tile_tangent_from_surface(
        sp,
        0.0,
        normal,
        transform,
        tile_mercator_coords,
    );
    let side = select(1.0, -1.0, negative_side);
    let extent = length(normal);
    let moved = outset * max(extent, 1.0) + side * line_offset;
    let line_units = line_scale.y * project_symbol_scale_from_cos_lat(sp.cos_lat);
    let offset = tangent * (moved * line_units / max(extent, 1e-6));
    let out_w = projected.clip_position.w + offset.w;
    let out_pos = vec4<f32>(
        projected.clip_position.xy + offset.xy,
        projected.horizon_distance * 0.5 * out_w,
        out_w,
    );

    return PlainGlobeLineVertexOutput(
        out_pos,
        color,
        normal / max(extent, 1.0),
        vec2<f32>(outset, inset),
        blur_width,
        position.x,
        clip_antimeridian,
    );
}
