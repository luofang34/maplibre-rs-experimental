struct MoonFrame {
    direction_radius: vec4<f32>,
    sun: vec4<f32>,
    transition: vec4<f32>,
};

@group(1) @binding(0) var<uniform> moon: MoonFrame;

struct VertexInput {
    @location(0) offset: vec2<f32>,
    @location(1) @interpolate(flat) right: vec3<f32>,
    @location(2) @interpolate(flat) up: vec3<f32>,
    @location(3) @interpolate(flat) forward: vec3<f32>,
};

const MOON_COLOR: vec3<f32> = vec3<f32>(0.92, 0.9, 0.85);
// Sunlight the Earth reflects onto the moon's night side, as a share of full sunlight.
const EARTHSHINE: f32 = 0.04;

@fragment
fn main(in: VertexInput) -> @location(0) vec4<f32> {
    let distance = length(in.offset);
    // About one pixel of antialiasing at the limb.
    let edge = max(fwidth(distance), 1e-4);
    let coverage = 1.0 - smoothstep(1.0 - edge, 1.0, distance);
    if coverage <= 0.0 {
        discard;
    }
    // The point of the sphere this pixel shows faces back along the line of sight.
    let depth = sqrt(max(1.0 - distance * distance, 0.0));
    let normal = in.offset.x * in.right + in.offset.y * in.up - depth * in.forward;
    let sunlight = select(1.0, max(dot(normal, moon.sun.xyz), 0.0), moon.sun.w > 0.5);
    let light = max(sunlight, EARTHSHINE);
    let alpha = coverage * moon.transition.x;
    return vec4<f32>(MOON_COLOR * light * alpha, alpha);
}
