// Depth probes: one point per probe, drawn into its own texel of a small target, writing
// whether the frame's depth at the probe's pixel lets the probe's depth through.
// depth_probe_multisampled.wgsl is the same for a multisampled frame.

@group(0) @binding(1) var frame_depth: texture_2d<f32>;

fn nearest_depth(at: vec2<i32>) -> f32 {
    return textureLoad(frame_depth, at, 0).r;
}

struct Probe {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) pixel: vec2<f32>,
    @location(1) @interpolate(flat) depth: f32,
};

struct Target {
    // Texels per row and rows of the result target, and the share of its distance a probe
    // is brought nearer, as overlay vertices are.
    width: f32,
    rows: f32,
    depth_bias: f32,
};

@group(0) @binding(0) var<uniform> probe_target: Target;

@vertex
fn vs(@builtin(vertex_index) index: u32, @location(0) pixel: vec2<f32>, @location(1) depth: f32) -> Probe {
    let width = u32(probe_target.width);
    let texel = vec2<f32>(f32(index % width), f32(index / width)) + 0.5;
    let ndc = vec2<f32>(texel.x / probe_target.width * 2.0 - 1.0, 1.0 - texel.y / probe_target.rows * 2.0);
    return Probe(vec4<f32>(ndc, 0.0, 1.0), pixel, depth);
}

@fragment
fn fs(in: Probe) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(frame_depth));
    let at = clamp(vec2<i32>(in.pixel), vec2<i32>(0, 0), size - vec2<i32>(1, 1));
    let seen = min(in.depth * (1.0 + probe_target.depth_bias), 1.0) >= nearest_depth(at);
    return vec4<f32>(select(0.0, 1.0, seen), 0.0, 0.0, 1.0);
}
