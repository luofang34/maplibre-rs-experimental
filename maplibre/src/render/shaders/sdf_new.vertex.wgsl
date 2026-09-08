// @include projection.vertex.wgsl
// @include symbol_uniforms.wgsl

struct VertexOutput {
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) kind: u32,
    @location(2) size: f32,
    @location(3) opacity: f32,
    @location(4) horizon_distance: f32,
    @builtin(position) position: vec4<f32>,
};

@group(2) @binding(0) var scene_depth: texture_depth_2d;

fn anchor_visibility(clip: vec4<f32>, viewport: vec2<f32>) -> f32 {
    if clip.w <= 0.0 { return 0.0; }
    let screen = (clip.xy / clip.w * vec2<f32>(0.5,-0.5) + vec2<f32>(0.5)) * viewport;
    let pixel = clamp(vec2<i32>(screen),vec2<i32>(0),vec2<i32>(textureDimensions(scene_depth))-vec2<i32>(1));
    // A pixel covers a footprint; the farthest adjacent sample avoids self-occluding a ground anchor.
    var surface = textureLoad(scene_depth,pixel,0);
    let limit = vec2<i32>(textureDimensions(scene_depth))-vec2<i32>(1);
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            surface = min(surface,textureLoad(scene_depth,clamp(pixel+vec2<i32>(x,y),vec2<i32>(0),limit),0));
        }
    }
    let anchor = clip.z / clip.w;
    return select(0.0,1.0,anchor + max(abs(anchor)*2e-3,1e-8) >= surface);
}

@vertex
fn main(
    @location(0) a_pos_offset: vec4<i32>,
    @location(1) a_data: vec4<u32>,
    @location(2) a_pixeloffset: vec4<i32>,
    @location(3) tile_mercator_coords: vec4<f32>,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
    @location(8) viewport_width: f32,
    @location(9) zoom_factor: f32,
    @location(11) viewport_height: f32,
    @location(12) feature: vec2<f32>,
) -> VertexOutput {
    let is_text = a_data.z == 0u;
    let metrics = select(symbol.icon, symbol.text, is_text);
    let alignment = select(symbol.icon_layout, symbol.text_layout, is_text);
    let anchor = vec2<f32>(a_pos_offset.xy);
    let elevation = feature.y * alignment.w + bitcast<f32>(a_pixeloffset.z);
    let transform = mat4x4<f32>(translate1, translate2, translate3, translate4);
    let projected = project_tile_position_3d(vec3<f32>(anchor, elevation), transform, tile_mercator_coords);
    let distance_ratio = select(projection.transition_and_padding.y / max(projected.clip_position.w, 1e-6),
        projected.clip_position.w / max(projection.transition_and_padding.y, 1e-6), alignment.x > 0.5);
    let perspective_ratio = clamp(0.5 + 0.5 * distance_ratio, 0.0, 4.0);
    let size = metrics.x * perspective_ratio;
    let scale = select(size, size / 24.0, is_text);
    var angle = alignment.z + bitcast<f32>(a_pixeloffset.w);
    if alignment.y > 0.5 {
        let tangent = project_tile_position_3d(vec3<f32>(anchor + vec2<f32>(cos(angle), sin(angle)) * 16.0, elevation), transform, tile_mercator_coords).clip_position;
        let delta = (tangent.xy / tangent.w - projected.clip_position.xy / projected.clip_position.w)
            * vec2<f32>(viewport_width, -viewport_height);
        if alignment.x < 0.5 { angle = atan2(delta.y, delta.x); }
        let keep_upright = select(symbol.placement.w, symbol.placement.z, is_text);
        if keep_upright > 0.5 && delta.x < 0.0 { angle += PROJECTION_PI; }
    }
    let rotation = mat2x2<f32>(cos(angle), sin(angle), -sin(angle), cos(angle));
    let offset = rotation * (vec2<f32>(a_pos_offset.zw) / 32.0 * scale + vec2<f32>(a_pixeloffset.xy) / 16.0);
    var position = projected.clip_position;
    if alignment.x > 0.5 {
        let tile_offset = offset * 8.0 * zoom_factor * project_symbol_scale(anchor.y, tile_mercator_coords);
        position = project_tile_position_3d(vec3<f32>(anchor + tile_offset, elevation), transform, tile_mercator_coords).clip_position;
    } else {
        position.x += offset.x * 2.0 / viewport_width * position.w;
        position.y -= offset.y * 2.0 / viewport_height * position.w;
    }
    // A small relative bias resolves coplanar labels without lifting them above unrelated hills.
    position.z += max(abs(position.z) * 2e-5, 1e-10);
    var near_visibility = 1.0;
    if alignment.x > 0.5 {
        let radius = bitcast<f32>(a_data.w) * scale * 8.0 * zoom_factor
            * project_symbol_scale(anchor.y, tile_mercator_coords);
        let x = project_tile_position_3d(vec3<f32>(anchor + vec2<f32>(radius,0.0),elevation),transform,tile_mercator_coords).clip_position;
        let y = project_tile_position_3d(vec3<f32>(anchor + vec2<f32>(0.0,radius),elevation),transform,tile_mercator_coords).clip_position;
        let depth_radius = abs(x.w - projected.clip_position.w) + abs(y.w - projected.clip_position.w);
        near_visibility = select(0.0,1.0,projected.clip_position.w - depth_radius > 0.0);
    }
    let visibility = feature.x * metrics.w * near_visibility * anchor_visibility(projected.clip_position,vec2<f32>(viewport_width,viewport_height));
    // Hidden geometry must not cross the eye plane and produce unbounded clipped triangles.
    if visibility <= 0.0 || projected.clip_position.w <= 0.0 { position = vec4<f32>(0.0, 0.0, 0.0, 1.0); }
    return VertexOutput(vec2<f32>(a_data.xy) / symbol.atlas.xy, a_data.z, size,
        visibility, projected.horizon_distance, position);
}
