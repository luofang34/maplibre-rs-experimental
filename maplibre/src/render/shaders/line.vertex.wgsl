// @include projection.vertex.wgsl
// @include line_flat.wgsl

// The direction in pixels in which a line's edge lies from its centre at `center`, for the
// tile-space extrusion `normal`. The inverse transpose of the projection's Jacobian carries a
// normal of the line on the ground to a normal of its image on the screen.
fn screen_normal(
    center: vec4<f32>,
    east: vec4<f32>,
    south: vec4<f32>,
    normal: vec2<f32>,
    half_viewport: vec2<f32>,
) -> vec2<f32> {
    let w = max(center.w, 1e-6);
    let x = (east.xy * w - center.xy * east.w) / (w * w) * half_viewport;
    let y = (south.xy * w - center.xy * south.w) / (w * w) * half_viewport;
    let determinant = x.x * y.y - y.x * x.y;
    let direction = vec2<f32>(y.y * normal.x - x.y * normal.y, x.x * normal.y - y.x * normal.x)
        * select(1.0, -1.0, determinant < 0.0);
    return direction / max(length(direction), 1e-12);
}

// How the clip position of a tile position changes per metre of height.
fn project_tile_up_3d(
    tile_position: vec3<f32>,
    fallback_matrix: mat4x4<f32>,
    tile_mercator_coords: vec4<f32>,
) -> vec4<f32> {
    let surface = tile_position_on_unit_sphere(tile_position.xy, tile_mercator_coords);
    let globe = projection.main_matrix * vec4<f32>(surface / projection.transition_and_padding.z, 0.0);
    let flat = fallback_matrix * vec4<f32>(0.0, 0.0, 1.0, 0.0);
    return blend_clip(flat, globe, projection.transition_and_padding.x);
}

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
    @location(8) progress: f32,
    @location(9) across: f32,
    @location(10) blur: f32,
    @location(11) @interpolate(flat) floor_width: f32,
    // Positions in the bound terrain tile and in the clip region of the line's tile, scaled to
    // tile units; a line on the terrain draws only where both lie inside.
    @location(12) tile_bounds: vec4<f32>,
};

@vertex
fn main(
    @location(0) position: vec2<f32>,
    @location(1) path: vec4<f32>,
    @location(2) tile_mercator_coords: vec4<f32>,
    // Offset, gap width and blur of the line in pixels.
    @location(3) line_style: vec4<f32>,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
    @location(8) color: vec4<f32>,
    @location(9) line_scale: vec2<f32>,
    @location(10) packed_style: f32,
    @location(11) viewport: vec2<f32>,
    // The square of the tile this draw covers, as minimum and maximum tile coordinates.
    @location(12) clip_region: vec4<f32>,
    @location(13) line_width: f32,
    @location(14) clip_antimeridian: u32,
    @location(15) layer_translate: vec2<f32>,
) -> VertexOutput {
    let normal = path.xy;
    // A width or offset that varies by feature rides in the mantissa of `packed_style`.
    let payload = bitcast<u32>(packed_style) & 0x7fffffu;
    let per_feature = payload != 0u;
    let feature_width = f32((payload >> 11u) & 0xfffu) / 16.0;
    let feature_offset = f32(i32(payload & 0x7ffu) - 1024) / 8.0;
    let line_width_px = select(line_width, feature_width, per_feature) * line_scale.x;
    // A gradient line measures its dashes in pixels rather than line widths.
    let dash_unit = select(line_width_px, 1.0, line_style.w > 0.5);
    let line_offset = select(line_style.x, feature_offset, per_feature);
    // A stroke's elevation slot carries its side and, in the mantissa, the gap width and blur of
    // its feature in eighths of a pixel; all ones leaves the layer's own.
    // A spatial line has a real elevation there instead, and keeps the layer's values.
    let has_stroke_bits = path.w <= -1e20;
    let stroke_bits = select(0x3fffffu, bitcast<u32>(path.w) & 0x7fffffu, has_stroke_bits);
    let negative_side = has_stroke_bits && ((stroke_bits >> 22u) & 1u) == 1u;
    let gap_bits = (stroke_bits >> 11u) & 0x7ffu;
    let blur_bits = stroke_bits & 0x7ffu;
    let gap_width = select(f32(gap_bits) / 8.0, line_style.y, gap_bits == 0x7ffu);
    let blur_width = select(f32(blur_bits) / 8.0, line_style.z, blur_bits == 0x7ffu);
    let gapwidth = gap_width * 0.5;

    let halfwidth = line_width_px * 0.5;
    let pixel_ratio = 1.0;
    let antialiasing = (1.0 / pixel_ratio) * 0.5;

    let inset = gapwidth + select(0.0, antialiasing, gapwidth > 0.0);
    let outset = gapwidth + halfwidth * select(1.0, 2.0, gapwidth > 0.0) +
                 select(0.0, antialiasing, halfwidth != 0.0);

    let transform = mat4x4<f32>(translate1, translate2, translate3, translate4);

    let spatial = path.w > -1e20;
    let ground = line_ground(position + layer_translate, tile_mercator_coords);
    let elevation = select(select(0.0, path.w, spatial), ground.elevation, ground.on_terrain);
    let projected_center = project_tile_position_3d(
        vec3<f32>(position + layer_translate, elevation),
        transform,
        tile_mercator_coords,
    );
    let tangent = project_tile_tangent_3d(
        vec3<f32>(position + layer_translate, elevation),
        normal,
        transform,
        tile_mercator_coords,
    );
    var center = projected_center.clip_position;
    // The offset moves both edges of the line the same way: the sign of the side undoes the
    // opposite directions of their extrusions.
    let side = select(1.0, -1.0, negative_side);
    // A join vertex carries the miter or bevel offset as a normal longer or shorter than one,
    // and the edge lies that much further out along it.
    let extent = length(normal);
    let moved = outset * max(extent, 1.0) + side * line_offset;
    if spatial {
        // Decks share the map-space width of adjoining draped roads, including foreshortening.
        // Casing and deck share the centerline tangent plane. Independently projecting
        // different widths onto a curved sphere gives them intersecting depth surfaces.
        center += tangent * (outset * line_scale.y);
        // A small terrain bias keeps sampled road endpoints above the shared surface.
        center.z += max(abs(center.z), 1e-8) * 2e-5;
    } else if ground.on_terrain {
        // The edges lie the line's width in pixels from its centre on the screen, however far
        // away the ground is. The tangents follow the slope of the surface, so the edges stay
        // square to the line where it climbs.
        let position_3d = vec3<f32>(position + layer_translate, elevation);
        let up = project_tile_up_3d(position_3d, transform, tile_mercator_coords);
        let east = project_tile_tangent_3d(position_3d, vec2<f32>(1.0, 0.0), transform, tile_mercator_coords)
            + up * ground.slope.x;
        let south = project_tile_tangent_3d(position_3d, vec2<f32>(0.0, 1.0), transform, tile_mercator_coords)
            + up * ground.slope.y;
        let half_viewport = viewport * 0.5;
        let direction = screen_normal(center, east, south, normal, half_viewport);
        center = vec4<f32>(center.xy + direction * moved / half_viewport * center.w, center.zw);
        // The centre lies on the mesh, but the ground under a pixel-wide edge may rise toward
        // the eye; a bias of a fraction of the distance keeps the line above it.
        center.z += max(abs(center.z), 1e-8) * TERRAIN_LINE_DEPTH_BIAS;
    } else {
        // The extrusion lies on the map, so it shrinks with distance as the ground does.
        let offset = tangent * (moved * line_scale.y / max(extent, 1e-6));
        center = vec4<f32>(center.xy + offset.xy, 0.0, center.w + offset.w);
    }

    // A line with a gradient counts its dashes along the progress of the whole line, scaled to
    // a range of 32767, in widths rounded down: a pattern repeats every `sum * 16 * floor(width)`
    // of those units, as GL JS draws it.
    let floor_width = max(floor(select(line_width, feature_width, per_feature)), 1.0);
    let gradient_dash = vec2<f32>(
        path.z / max(color.x, 1e-6) * 32767.0 / (16.0 * floor_width),
        color.x / max(line_scale.y, 1e-6) * 16.0 * floor_width / 32767.0,
    );
    return VertexOutput(
        center,
        color,
        normal / max(extent, 1.0),
        vec2<f32>(outset, inset),
        // A deck and a line on the terrain keep a one-pixel feather; zero lets a line on the map
        // scale its feather with how much it has narrowed, as GL JS's gamma scale does.
        select(0.0, 1.0, spatial || ground.on_terrain),
        projected_center.horizon_distance,
        position.x,
        clip_antimeridian,
        select(
            vec2<f32>(path.z / max(line_scale.y * dash_unit, 1e-6), dash_unit),
            gradient_dash,
            line_style.w > 0.5,
        ),
        // A gradient layer carries the length of the line in the red channel of its colour.
        path.z / max(color.x, 1e-6),
        // Which side of the line the vertex is on: the elevation sentinel of a stroke encodes it.
        select(1.0, -1.0, negative_side) * min(length(normal), 1.0),
        blur_width,
        floor_width,
        select(
            vec4<f32>(2048.0),
            vec4<f32>(
                ground.terrain_position,
                (position - clip_region.xy) * 4096.0 / (clip_region.zw - clip_region.xy),
            ),
            ground.on_terrain,
        ),
    );
}

// The fraction of its distance by which a line on the terrain is drawn nearer than the
// surface beneath it.
const TERRAIN_LINE_DEPTH_BIAS: f32 = 2e-3;
