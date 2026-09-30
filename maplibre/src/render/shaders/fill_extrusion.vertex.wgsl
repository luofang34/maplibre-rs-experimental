// @include projection.vertex.wgsl

struct VertexOutput {
    @location(0) v_color: vec4<f32>,
    @location(4) horizon_distance: f32,
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
    if abs(normal.y) * 16384.0 >= 0.5 {
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
    return VertexOutput(vec4<f32>(rgb, 1.0) * paint.x, projected.horizon_distance, projected.clip_position);
}
