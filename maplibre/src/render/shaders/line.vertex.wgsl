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
fn project_tile_up_from_surface(
    surface: vec3<f32>,
    fallback_matrix: mat4x4<f32>,
) -> vec4<f32> {
    let transition = projection.transition_and_padding.x;
    if transition <= 0.0 {
        return fallback_matrix * vec4<f32>(0.0, 0.0, 1.0, 0.0);
    }
    let globe = projection.main_matrix * vec4<f32>(surface / projection.transition_and_padding.z, 0.0);
    if transition >= 1.0 {
        return globe;
    }
    let flat = fallback_matrix * vec4<f32>(0.0, 0.0, 1.0, 0.0);
    return blend_clip(flat, globe, transition);
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
    @location(0) v_pos_q2: vec2<i32>,
    @location(1) v_norm_packed: u32,
    @location(2) tile_mercator_coords: vec4<f32>,
    // Offset, gap width and blur of the line in pixels.
    @location(3) line_style: vec4<f32>,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
    @location(8) color: vec4<f32>,
    @location(9) line_scale: vec2<f32>,
    @location(10) v_meta: vec2<u32>,
    @location(11) viewport: vec2<f32>,
    // The square of the tile this draw covers, as minimum and maximum tile coordinates.
    @location(12) clip_region: vec4<f32>,
    @location(13) line_width: f32,
    @location(14) clip_antimeridian: u32,
    @location(15) layer_translate: vec2<f32>,
) -> VertexOutput {
    let position = vec2<f32>(v_pos_q2) * 0.25 + vec2<f32>(2048.0);
    let normal = unpack2x16float(v_norm_packed);
    let path_dist = f32(bitcast<i32>(v_meta.x << 8u) >> 8u) * 0.125;
    let w3 = v_meta.y;
    let style_flags = u32(line_style.w + 0.5);
    let is_gradient = (style_flags & 1u) != 0u;
    let elevated_line = (style_flags & 2u) != 0u;
    let spatial = (w3 >> 31u) != 0u;
    let negative_side = ((w3 >> 30u) & 1u) != 0u;
    let per_feature = ((w3 >> 29u) & 1u) != 0u;
    let feature_width = f32((w3 >> 18u) & 0x7ffu) / 16.0;
    let feature_offset = f32(i32((w3 >> 8u) & 0x3ffu) - 512) / 8.0;
    let line_width_px = select(line_width, feature_width, per_feature) * line_scale.x;
    // A gradient line measures its dashes in pixels rather than line widths.
    let dash_unit = select(line_width_px, 1.0, is_gradient);
    let line_offset = select(line_style.x, feature_offset, per_feature);
    let gap_u8 = select(w3 & 0xffu, 0xffu, spatial);
    let blur_u8 = select((v_meta.x >> 24u) & 0xffu, 0xffu, spatial);
    let gap_width = select(f32(gap_u8) / 8.0, line_style.y, gap_u8 == 0xffu);
    let blur_width = select(f32(blur_u8) / 8.0, line_style.z, blur_u8 == 0xffu);
    let gapwidth = gap_width * 0.5;

    let halfwidth = line_width_px * 0.5;
    let pixel_ratio = 1.0;
    let antialiasing = (1.0 / pixel_ratio) * 0.5;

    let inset = gapwidth + select(0.0, antialiasing, gapwidth > 0.0);
    let outset = gapwidth + halfwidth * select(1.0, 2.0, gapwidth > 0.0) +
                 select(0.0, antialiasing, halfwidth != 0.0);

    let transform = mat4x4<f32>(translate1, translate2, translate3, translate4);

    let elev_u16 = (w3 & 0xffu) | (((v_meta.x >> 24u) & 0xffu) << 8u);
    let spatial_elevation = f32(elev_u16) * 0.25 - 1000.0;
    let pos_2d = position + layer_translate;
    let ground = line_ground(pos_2d, tile_mercator_coords);
    let elevation = select(select(0.0, spatial_elevation, spatial), ground.elevation, ground.on_terrain);
    let position_3d = vec3<f32>(pos_2d, elevation);
    let is_globe = projection.transition_and_padding.x > 0.0;
    var sp = GlobeSurfacePoint(vec3<f32>(0.0), 0.0, 1.0, 1.0, false, false);
    var center = vec4<f32>(0.0);
    var tangent = vec4<f32>(0.0);
    var horizon_distance = 1.0;
    if is_globe {
        sp = tile_surface_point(pos_2d, tile_mercator_coords);
        let projected_center = project_tile_position_3d_from_surface(
            sp,
            position_3d,
            transform,
            tile_mercator_coords,
        );
        center = projected_center.clip_position;
        horizon_distance = projected_center.horizon_distance;
        tangent = project_tile_tangent_from_surface(
            sp,
            elevation,
            normal,
            transform,
            tile_mercator_coords,
        );
    } else {
        center = transform * vec4<f32>(position_3d, 1.0);
        tangent = transform * vec4<f32>(normal, 0.0, 0.0);
    }
    // The offset moves both edges of the line the same way: the sign of the side undoes the
    // opposite directions of their extrusions.
    let side = select(1.0, -1.0, negative_side);
    // A join vertex carries the miter or bevel offset as a normal longer or shorter than one,
    // and both the half-width outset and the polyline offset travel along that join normal so
    // offset corners stay parallel to their adjoining segments without inside pinches or clipped
    // outside miters. Cap the offset's miter multiplier to prevent unbounded concave spikes.
    let extent = length(normal);
    let miter = max(extent, 1.0);
    let offset_miter = min(miter, 4.0);
    let moved = outset * miter + side * line_offset * offset_miter;
    if spatial && elevated_line {
        // Elevated 3D paths (`line-elevation`) keep crisp screen-pixel widths at all pitches and
        // when viewed from above, eye level, or below by rotating the projected path tangent 90°
        // in screen pixels (without flipping across the horizontal plane's horizon).
        var east: vec4<f32>;
        var south: vec4<f32>;
        if is_globe {
            east = project_tile_tangent_from_surface(sp, elevation, vec2<f32>(1.0, 0.0), transform, tile_mercator_coords);
            south = project_tile_tangent_from_surface(sp, elevation, vec2<f32>(0.0, 1.0), transform, tile_mercator_coords);
        } else {
            east = transform * vec4<f32>(1.0, 0.0, 0.0, 0.0);
            south = transform * vec4<f32>(0.0, 1.0, 0.0, 0.0);
        }
        let half_viewport = viewport * 0.5;
        let w = max(center.w, 1e-6);
        let east_px = (east.xy * w - center.xy * east.w) / (w * w) * half_viewport;
        let south_px = (south.xy * w - center.xy * south.w) / (w * w) * half_viewport;
        let unit_n = normal / max(extent, 1e-6);
        let tangent_px = east_px * (-unit_n.y) + south_px * unit_n.x;
        let perp_px = vec2<f32>(-tangent_px.y, tangent_px.x);
        let perp_len = length(perp_px);
        let direction = select(vec2<f32>(1.0, 0.0), perp_px / max(perp_len, 1e-12), perp_len > 1e-9);
        center = vec4<f32>(center.xy + direction * moved / half_viewport * center.w, center.zw);
        center.z += max(abs(center.z), 1e-8) * 5e-5;
    } else if spatial {
        // Decks share the map-space width of adjoining draped roads, including foreshortening.
        // Casing and deck share the centerline tangent plane. Independently projecting
        // different widths onto a curved sphere gives them intersecting depth surfaces.
        center += tangent * (moved * line_scale.y / max(extent, 1e-6));
        // A small terrain bias keeps sampled road endpoints above the shared surface.
        center.z += max(abs(center.z), 1e-8) * 2e-5;
    } else if ground.on_terrain {
        // The edges lie the line's width in pixels from its centre on the screen, however far
        // away the ground is. The tangents follow the slope of the surface, so the edges stay
        // square to the line where it climbs.
        let up = project_tile_up_from_surface(sp.surface, transform);
        var east: vec4<f32>;
        var south: vec4<f32>;
        if is_globe {
            east = project_tile_tangent_from_surface(sp, elevation, vec2<f32>(1.0, 0.0), transform, tile_mercator_coords);
            south = project_tile_tangent_from_surface(sp, elevation, vec2<f32>(0.0, 1.0), transform, tile_mercator_coords);
        } else {
            east = transform * vec4<f32>(1.0, 0.0, 0.0, 0.0);
            south = transform * vec4<f32>(0.0, 1.0, 0.0, 0.0);
        }
        east += up * ground.slope.x;
        south += up * ground.slope.y;
        let half_viewport = viewport * 0.5;
        let direction = screen_normal(center, east, south, normal, half_viewport);
        center = vec4<f32>(center.xy + direction * moved / half_viewport * center.w, center.zw);
        // The centre lies on the mesh, but the ground under a pixel-wide edge may rise toward
        // the eye; a bias of a fraction of the distance keeps the line above it.
        center.z += max(abs(center.z), 1e-8) * TERRAIN_LINE_DEPTH_BIAS;
    } else {
        // The extrusion lies on the map, so it shrinks with distance as the ground does.
        var line_units = line_scale.y;
        if is_globe {
            line_units *= project_symbol_scale_from_cos_lat(sp.cos_lat);
        }
        let offset = tangent * (moved * line_units / max(extent, 1e-6));
        let out_w = center.w + offset.w;
        center = vec4<f32>(center.xy + offset.xy, horizon_distance * 0.5 * out_w, out_w);
    }

    let dash_scale = line_scale.y * select(
        mix(1.0, projection.globe_circle.x, projection.transition_and_padding.x),
        1.0,
        spatial || ground.on_terrain,
    );

    // A line with a gradient counts its dashes along the progress of the whole line, scaled to
    // a range of 32767, in widths rounded down: a pattern repeats every `sum * 16 * floor(width)`
    // of those units, as GL JS draws it.
    let floor_width = max(floor(select(line_width, feature_width, per_feature)), 1.0);
    var dash = vec2<f32>(path_dist / max(dash_scale * dash_unit, 1e-6), dash_unit);
    var progress = 0.0;
    if is_gradient {
        let grad_len_q4 = round(color.x * 255.0)
            + round(color.y * 255.0) * 256.0
            + round(color.z * 255.0) * 65536.0;
        let grad_len = max(grad_len_q4 / 16.0, 1e-6);
        dash = vec2<f32>(
            path_dist / grad_len * 32767.0 / (16.0 * floor_width),
            grad_len / max(dash_scale, 1e-6) * 16.0 * floor_width / 32767.0,
        );
        progress = path_dist / grad_len;
    }
    return VertexOutput(
        center,
        color,
        normal / max(extent, 1.0),
        vec2<f32>(outset, inset),
        // A deck and a line on the terrain keep a one-pixel feather; zero lets a line on the map
        // scale its feather with how much it has narrowed, as GL JS's gamma scale does.
        select(0.0, 1.0, spatial || ground.on_terrain),
        horizon_distance,
        position.x,
        clip_antimeridian,
        dash,
        // A gradient layer carries the 24-bit fixed-point length of the line in `color.xyz`.
        progress,
        // Which side of the line the vertex is on: the elevation sentinel of a stroke encodes it.
        select(1.0, -1.0, negative_side) * min(extent, 1.0),
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
