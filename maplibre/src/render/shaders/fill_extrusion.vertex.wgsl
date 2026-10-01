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

@vertex
fn main(
    @location(0) position: vec2<f32>,
    // The wall's outward normal, twice as long on the top edge; zero on the roof.
    @location(1) wall: vec2<f32>,
    @location(2) tile_mercator_coords: vec4<f32>,
    @location(3) base_in: f32,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
    @location(8) color: vec4<f32>,
    @location(9) zoom_factor: f32,
    @location(10) edge_distance: f32,
    @location(11) height: f32,
    @location(12) light_color: vec4<f32>,
    // Opacity, vertical gradient and light intensity.
    @location(13) paint: vec4<f32>,
    @location(14) light_position: vec4<f32>,
    @location(15) layer_translate: vec2<f32>,
) -> VertexOutput {
    let length_of_wall = length(wall);
    let is_wall = length_of_wall > 0.5;
    let top = select(1.0, select(0.0, 1.0, length_of_wall > 1.5), is_wall);
    let normal = select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(wall / max(length_of_wall, 1e-6), 0.0), is_wall);
    let z = select(base_in, height, top > 0.5);
    let projected = project_tile_position_3d(
        vec3<f32>(position + layer_translate, z),
        mat4x4<f32>(translate1, translate2, translate3, translate4),
        tile_mercator_coords,
    );

    let intensity = paint.z;
    // The alpha of the colour darkens it rather than making the extrusion translucent.
    let surface = color.rgb * color.a;
    let colorvalue = dot(surface, vec3<f32>(0.2126, 0.7152, 0.0722));
    // A little ambient light keeps every extrusion from going black.
    let lit = surface + vec3<f32>(0.03);
    var directional = clamp(dot(normal, light_position.xyz), 0.0, 1.0);
    directional = mix(1.0 - intensity, max(1.0 - colorvalue + intensity, 1.0), directional);
    if is_wall {
        let gradient = clamp(
            (top + base_in) * pow(height / 150.0, 0.5),
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

    // The pattern is fixed to the pixel grid of the map at the view zoom. A roof takes its
    // place in the tile; a wall takes its distance along the ring and its height.
    let tile_units_to_pixels = 1.0 / (8.0 * zoom_factor);
    let tile_scale = 1.0 / (tile_mercator_coords.z * 4096.0);
    // A wall's pattern rows span half as many metres as the tile-unit conversion alone gives, which
    // is how GL JS's walls repeat the image.
    let height_factor = -0.5 * tile_scale / 4096.0;
    let pattern_place = select(position, vec2<f32>(edge_distance, z * height_factor), is_wall);
    let view_scale = tile_scale / zoom_factor;
    // The corner of the tile in map pixels, split as GL JS splits it so that taking it modulo
    // the pattern size loses nothing: the tile's column and row are exact integers, and
    // scaling them by powers of two is exact too.
    let corner = floor(tile_mercator_coords.xy * tile_scale + vec2<f32>(0.5)) * 512.0 * (view_scale / tile_scale);
    let origin_upper = floor(corner / 65536.0);
    let origin_lower = corner - origin_upper * 65536.0;
    let pattern_light = mix(1.0 - intensity, max(0.5 + intensity, 1.0), clamp(dot(normal, light_position.xyz), 0.0, 1.0));
    var pattern_directional = pattern_light;
    if is_wall {
        let gradient = clamp(
            (top + base_in) * pow(height / 150.0, 0.5),
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
        vec4<f32>(rgb, 1.0) * paint.x,
        projected.horizon_distance,
        pattern_place * tile_units_to_pixels,
        origin_upper,
        origin_lower,
        vec4<f32>(pattern_rgb, 1.0) * paint.x,
        projected.clip_position,
    );
}
