struct VertexInput {
    @location(0) surface: vec3<f32>,
    @location(1) surface_distance: f32,
    @location(2) atmosphere_blend: f32,
    @location(3) view_direction: vec3<f32>,
};

@fragment
fn main(in: VertexInput) -> @location(0) vec4<f32> {
    if in.surface_distance < -0.03 {
        discard;
    }
    let normal = normalize(in.surface);
    let sun_direction = normalize(vec3<f32>(-0.4, 0.6, 1.0));
    let limb = pow(1.0 - clamp(dot(normal, in.view_direction), 0.0, 1.0), 1.5);
    let sunlight = 0.35 + 0.65 * max(dot(normal, sun_direction), 0.0);
    let color = vec3<f32>(0.20, 0.48, 1.0) * sunlight;
    let alpha = in.atmosphere_blend * (0.06 + 0.5 * limb);
    return vec4<f32>(color, alpha);
}
