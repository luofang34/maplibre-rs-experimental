// Direction to the moon in globe world axes, then the tangent of its angular radius; the sun's
// direction, then 1 when a sun lights the phase; the globe transition, the moon's brightness
// relative to a full moon, and the halo's reach in disc radii; the Earth's shadow's axis and its
// umbra's radius in radians; its penumbra's radius, the umbra's Danjon value, and 1 while a
// shadow is set.
struct MoonFrame {
    direction_radius: vec4<f32>,
    sun: vec4<f32>,
    transition_flux_reach: vec4<f32>,
    shadow_umbra: vec4<f32>,
    penumbra_danjon: vec4<f32>,
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
// The umbra's light, refracted through the Earth's atmosphere, from Danjon's L0, dark brown and
// barely seen, to L4, bright copper, as drawn at the umbra's centre. The levels are what an eye
// adapted to the night sees rather than the umbra's share of sunlight, a few ten-thousandths, so
// that an L2 moon stays a plain deep red disc, as it does to the naked eye.
const UMBRA_DARK: vec3<f32> = vec3<f32>(0.5, 0.22, 0.15);
const UMBRA_BRIGHT: vec3<f32> = vec3<f32>(1.0, 0.6, 0.35);
const UMBRA_LEVEL_DARK: f32 = 0.12;
const UMBRA_LEVEL_BRIGHT: f32 = 0.85;
// The umbra brightens towards its edge, where the light has crossed less of the atmosphere.
const UMBRA_EDGE_GAIN: f32 = 1.5;
// Light grazing the Earth's limb crosses the ozone layer, which takes its red and orange, so a
// turquoise band lines the umbra's edge. It reaches this share of the umbra's radius inward, is
// this bright at its middle, and takes this share of the copper there.
const OZONE_COLOR: vec3<f32> = vec3<f32>(0.3, 0.75, 0.8);
const OZONE_WIDTH: f32 = 0.15;
const OZONE_LEVEL: f32 = 0.35;
const OZONE_ABSORPTION: f32 = 0.7;

// The share of the sun's light that reaches the moon along `ray`, and the light the umbra
// refracts onto it there, copper within and turquoise along its edge; all sunlight and no umbra
// while no shadow is set.
fn earth_shadow(ray: vec3<f32>) -> vec4<f32> {
    if moon.penumbra_danjon.z < 0.5 {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let axis = moon.shadow_umbra.xyz;
    let umbra = moon.shadow_umbra.w;
    // The angle from the axis, kept precise at the fraction of a degree a shadow spans.
    let angle = atan2(length(cross(ray, axis)), dot(ray, axis));
    let direct = smoothstep(umbra, moon.penumbra_danjon.x, angle);
    let danjon = moon.penumbra_danjon.y / 4.0;
    let edge = smoothstep(0.0, umbra, angle);
    let level = mix(UMBRA_LEVEL_DARK, UMBRA_LEVEL_BRIGHT, pow(danjon, 1.2))
        * mix(1.0, UMBRA_EDGE_GAIN, edge * edge);
    let inside = (umbra - angle) / (umbra * OZONE_WIDTH);
    let band = smoothstep(0.0, 0.5, inside) * (1.0 - smoothstep(0.5, 1.0, inside));
    // The ozone takes the red from the copper light it lines, and leaves its turquoise.
    let copper = mix(UMBRA_DARK, UMBRA_BRIGHT, danjon) * level * (1.0 - OZONE_ABSORPTION * band);
    let umbral = (copper + OZONE_COLOR * OZONE_LEVEL * band) * (1.0 - direct);
    return vec4<f32>(umbral, direct);
}

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
    // The Earth's shadow takes the sunlight; inside it the Earth shows the moon its night side,
    // so the earthshine goes too, and the umbra's refracted light is all that is left.
    let ray = normalize(in.forward
        + moon.direction_radius.w * (in.offset.x * in.right + in.offset.y * in.up));
    let shadow = earth_shadow(ray);
    let surface = max(1.0 - exp(-EXPOSURE * lit * shadow.w), EARTHSHINE * shadow.w);
    // The halo adds light around the disc, falling with the square of the angle and fading out
    // before the quad's edge, as bright as the moon is.
    let halo = flux * HALO_STRENGTH / max(distance * distance, 1.0)
        * (1.0 - smoothstep(0.5 * reach, reach, distance));
    let alpha = coverage * transition;
    let color = (MOON_COLOR * surface + shadow.rgb) * alpha
        + HALO_COLOR * halo * transition * (1.0 - coverage);
    if alpha <= 0.0 && halo <= 0.0 {
        discard;
    }
    return vec4<f32>(color, alpha);
}
