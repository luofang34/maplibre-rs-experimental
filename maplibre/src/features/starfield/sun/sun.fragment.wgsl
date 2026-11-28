struct SunFrame {
    direction_radius: vec4<f32>,
    viewport_glare_transition: vec4<f32>,
};

@group(1) @binding(0) var<uniform> sun: SunFrame;

struct VertexInput {
    @location(0) offset: vec2<f32>,
    @location(1) @interpolate(flat) visible: f32,
};

const SUN_COLOR: vec3<f32> = vec3<f32>(1.0, 0.97, 0.9);
const GLARE_COLOR: vec3<f32> = vec3<f32>(1.0, 0.85, 0.6);
// The glare's brightness one disc radius out, with the whole disc showing.
const GLARE_STRENGTH: f32 = 0.35;
// The sun's limb darkening: a linear law, I(mu) = 1 - u (1 - mu), with u as the visible band.
const LIMB_DARKENING: f32 = 0.6;

@fragment
fn main(in: VertexInput) -> @location(0) vec4<f32> {
    let distance = length(in.offset);
    let transition = sun.viewport_glare_transition.w;
    let reach = sun.viewport_glare_transition.z;
    // One physical pixel of antialiasing at the limb.
    let edge = 1.0 / max(sun.direction_radius.w, 1.0);
    let disc = 1.0 - smoothstep(1.0 - edge, 1.0, distance);
    let mu = sqrt(max(1.0 - distance * distance, 0.0));
    let limb = 1.0 - LIMB_DARKENING * (1.0 - mu);
    // Glare falls with the square of the distance and fades out before the quad's edge; it
    // adds light without covering what lies behind it.
    let glare = in.visible * GLARE_STRENGTH / max(distance * distance, 1.0)
        * (1.0 - smoothstep(0.5 * reach, reach, distance));
    let alpha = disc * transition;
    let color = SUN_COLOR * limb * alpha + GLARE_COLOR * glare * transition * (1.0 - disc);
    return vec4<f32>(color, alpha);
}
