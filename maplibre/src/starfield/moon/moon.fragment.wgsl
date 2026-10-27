// Direction to the moon in globe world axes, then the tangent of its angular radius; the sun's
// direction, then 1 when a sun lights the phase; the globe transition, the moon's brightness
// relative to a full moon, and the halo's reach in disc radii.
struct MoonFrame {
    direction_radius: vec4<f32>,
    sun: vec4<f32>,
    transition_flux_reach: vec4<f32>,
};

@group(1) @binding(0) var<uniform> moon: MoonFrame;

struct VertexInput {
    @location(0) offset: vec2<f32>,
    @location(1) @interpolate(flat) right: vec3<f32>,
    @location(2) @interpolate(flat) up: vec3<f32>,
    @location(3) @interpolate(flat) forward: vec3<f32>,
};

// The full moon's near-white, a little warm.
const MOON_COLOR: vec3<f32> = vec3<f32>(0.98, 0.97, 0.93);
const HALO_COLOR: vec3<f32> = vec3<f32>(0.85, 0.88, 0.95);
// Sunlight the Earth reflects onto the moon's night side, as a share of full sunlight.
const EARTHSHINE: f32 = 0.04;
// How fast the lit surface saturates towards white: a full moon's 1 shows at 0.95.
const EXPOSURE: f32 = 3.0;
// The halo's brightness one disc radius out, around a full moon.
const HALO_STRENGTH: f32 = 0.4;

@fragment
fn main(in: VertexInput) -> @location(0) vec4<f32> {
    let distance = length(in.offset);
    let transition = moon.transition_flux_reach.x;
    let flux = moon.transition_flux_reach.y;
    let reach = moon.transition_flux_reach.z;
    // About one pixel of antialiasing at the limb.
    let edge = max(fwidth(distance), 1e-4);
    let coverage = 1.0 - smoothstep(1.0 - edge, 1.0, distance);
    // The point of the sphere this pixel shows faces back along the line of sight.
    let toward_eye = sqrt(max(1.0 - distance * distance, 0.0));
    let normal = in.offset.x * in.right + in.offset.y * in.up - toward_eye * in.forward;
    // The moon's regolith scatters as Lommel-Seeliger, mu0 / (mu0 + mu): a full moon is as
    // bright at its limb as at its centre. Doubled, a full moon reads 1 all over.
    let mu0 = select(toward_eye, max(dot(normal, moon.sun.xyz), 0.0), moon.sun.w > 0.5);
    let lit = 2.0 * mu0 / max(mu0 + toward_eye, 1e-4);
    let surface = max(1.0 - exp(-EXPOSURE * lit), EARTHSHINE);
    // The halo adds light around the disc, falling with the square of the angle and fading out
    // before the quad's edge, as bright as the moon is.
    let halo = flux * HALO_STRENGTH / max(distance * distance, 1.0)
        * (1.0 - smoothstep(0.5 * reach, reach, distance));
    let alpha = coverage * transition;
    let color = MOON_COLOR * surface * alpha + HALO_COLOR * halo * transition * (1.0 - coverage);
    if alpha <= 0.0 && halo <= 0.0 {
        discard;
    }
    return vec4<f32>(color, alpha);
}
