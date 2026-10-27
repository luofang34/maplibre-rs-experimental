// @include ../../render/shaders/projection.vertex.wgsl

// Direction to the sun in globe world axes, then its radius in physical pixels; the viewport
// in physical pixels, the glare's reach in disc radii, and the globe transition.
struct SunFrame {
    direction_radius: vec4<f32>,
    viewport_glare_transition: vec4<f32>,
};

@group(1) @binding(0) var<uniform> sun: SunFrame;
// The depth the main pass left; zero, the reversed-Z far plane, where nothing was drawn.
@group(2) @binding(0) var scene_depth: texture_2d<f32>;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    // Offset from the sun's centre in disc radii.
    @location(0) offset: vec2<f32>,
    // The share of the disc the frame's depth leaves showing, which the glare scales with.
    @location(1) @interpolate(flat) visible: f32,
};

const SAMPLE_RINGS: u32 = 2u;
const SAMPLES_PER_RING: u32 = 8u;

// The share of points across the disc at `centre` that nothing was drawn over. Points off the
// frame count as showing: no ground was drawn there to hide them.
fn visible_share(centre: vec2<f32>, radius: f32) -> f32 {
    let size = vec2<i32>(textureDimensions(scene_depth));
    var showing = 0.0;
    for (var ring = 0u; ring < SAMPLE_RINGS; ring++) {
        let distance = radius * (f32(ring) + 0.5) / f32(SAMPLE_RINGS);
        for (var index = 0u; index < SAMPLES_PER_RING; index++) {
            let angle = (f32(index) + 0.5 * f32(ring)) * PROJECTION_TWO_PI
                / f32(SAMPLES_PER_RING);
            let pixel = vec2<i32>(centre + distance * vec2<f32>(cos(angle), sin(angle)));
            let inside = all(pixel >= vec2<i32>(0)) && all(pixel < size);
            let depth = textureLoad(scene_depth, clamp(pixel, vec2<i32>(0), size - 1), 0).r;
            showing += select(1.0, select(0.0, 1.0, depth <= 1e-6), inside);
        }
    }
    return showing / f32(SAMPLE_RINGS * SAMPLES_PER_RING);
}

@vertex
fn main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0),
        vec2<f32>(-1.0, 1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
    );
    let reach = sun.viewport_glare_transition.z;
    let corner = corners[vertex_index] * reach;
    // A direction has no position: the sun turns with the camera and stays at infinity.
    let clip = projection.main_matrix * vec4<f32>(sun.direction_radius.xyz, 0.0);
    if clip.w <= 0.0 {
        return VertexOutput(vec4<f32>(2.0, 2.0, 2.0, 1.0), corner, 0.0);
    }
    let viewport = sun.viewport_glare_transition.xy;
    let radius = sun.direction_radius.w;
    let ndc = clip.xy / clip.w;
    let centre = vec2<f32>((ndc.x + 1.0) * 0.5 * viewport.x, (1.0 - ndc.y) * 0.5 * viewport.y);
    let offset = corner * radius * 2.0 / viewport;
    // Depth zero is the reversed-Z far plane, behind everything that writes depth.
    let position = vec4<f32>(clip.xy + offset * clip.w, 0.0, clip.w);
    return VertexOutput(position, corner, visible_share(centre, radius));
}
