// @include ../render/shaders/projection.vertex.wgsl
// @include ../render/shaders/atmosphere_scattering.wgsl.part

// x, y: the viewport in physical pixels; z: physical pixels per logical pixel; w: how far the
// map has turned into a globe, which the stars fade in with. Then the rotation from globe world
// axes into view axes, and the atmosphere's inputs: the globe's centre and the sun in view
// axes with the share of it an eclipse leaves as w, then the globe's radius in pixels, the atmosphere's blend and the body's radius; and
// the moon.
struct StarfieldFrame {
    viewport_ratio_transition: vec4<f32>,
    view_x: vec4<f32>,
    view_y: vec4<f32>,
    view_z: vec4<f32>,
    globe_position: vec4<f32>,
    sun_direction: vec4<f32>,
    radius_blend_body: vec4<f32>,
    // Direction to the moon in globe world axes, then its brightness relative to a full moon.
    moon: vec4<f32>,
};

// Sky brightness, as the atmosphere draw adds it, at which a star starts to fade and at which
// it is gone: a star shows against a night sky and not against a lit one.
const STAR_FADE_START: f32 = 0.02;
const STAR_FADE_END: f32 = 0.1;

// The sky a full moon brightens right beside it, and the angle in radians over which that
// falls to half: its light scattered in the air washes out the stars around it.
const MOON_SKY_BRIGHTNESS: f32 = 0.12;
const MOON_SKY_HALF_ANGLE: f32 = 0.07;

// The sky's brightness the moon adds in direction `direction`, as bright as the moon is.
fn moon_sky(direction: vec3<f32>) -> f32 {
    let brightness = starfield.moon.w;
    if brightness <= 0.0 {
        return 0.0;
    }
    let angle = acos(clamp(dot(normalize(direction), starfield.moon.xyz), -1.0, 1.0));
    let falloff = angle / MOON_SKY_HALF_ANGLE;
    return MOON_SKY_BRIGHTNESS * brightness / (1.0 + falloff * falloff);
}

// How much of a star shows through the sky in direction `direction`: the sky the atmosphere
// draws, and the moon's light scattered around it.
fn sky_visibility(direction: vec3<f32>) -> f32 {
    let moonlit = moon_sky(direction);
    let blend = starfield.radius_blend_body.y;
    if blend <= 0.0 {
        return 1.0 - smoothstep(STAR_FADE_START, STAR_FADE_END, moonlit);
    }
    let view_direction = mat3x3<f32>(
        starfield.view_x.xyz,
        starfield.view_y.xyz,
        starfield.view_z.xyz,
    ) * direction;
    let body_radius = starfield.radius_blend_body.z;
    let eye = -starfield.globe_position.xyz * body_radius / starfield.radius_blend_body.x;
    let raw = scattering(
        view_direction,
        eye,
        starfield.sun_direction.xyz,
        body_radius,
        starfield.sun_direction.w,
    );
    let sky = displayed_scattering(raw).rgb * blend;
    let luminance = dot(sky, vec3<f32>(0.2126, 0.7152, 0.0722)) + moonlit;
    return 1.0 - smoothstep(STAR_FADE_START, STAR_FADE_END, luminance);
}

@group(1) @binding(0) var<uniform> starfield: StarfieldFrame;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    // The corner of the star's square, from -1 to 1 across it.
    @location(0) corner: vec2<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
};

@vertex
fn main(
    @builtin(vertex_index) vertex_index: u32,
    @location(0) direction_radius: vec4<f32>,
    @location(1) color_brightness: vec4<f32>,
) -> VertexOutput {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0),
        vec2<f32>(-1.0, 1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
    );
    let corner = corners[vertex_index];
    // A direction has no position, so the view's translation drops out and its scale cancels
    // in the perspective divide: the star turns with the camera and stays at infinity.
    let clip = projection.main_matrix * vec4<f32>(direction_radius.xyz, 0.0);
    let frame = starfield.viewport_ratio_transition;
    if clip.w <= 0.0 {
        // Behind the eye: outside every clip volume.
        return VertexOutput(vec4<f32>(2.0, 2.0, 2.0, 1.0), corner, vec4<f32>(0.0));
    }
    let radius = direction_radius.w * frame.z;
    let offset = corner * radius * 2.0 / frame.xy;
    // Depth zero is the reversed-Z far plane, behind everything that writes depth.
    let position = vec4<f32>(clip.xy + offset * clip.w, 0.0, clip.w);
    let brightness = color_brightness.a * frame.w * sky_visibility(direction_radius.xyz);
    return VertexOutput(position, corner, vec4<f32>(color_brightness.rgb, brightness));
}
