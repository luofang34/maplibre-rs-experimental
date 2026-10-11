// Host overlay triangles: positions in render-target pixels, premultiplied colours.

struct Viewport {
    size: vec2<f32>,
    // Share of its distance a vertex is brought nearer before the depth test.
    depth_bias: f32,
};

@group(0) @binding(0) var<uniform> viewport: Viewport;

struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs(@location(0) position: vec2<f32>, @location(1) depth: f32, @location(2) color: vec4<f32>) -> Varyings {
    let ndc = vec2<f32>(position.x / viewport.size.x * 2.0 - 1.0, 1.0 - position.y / viewport.size.y * 2.0);
    // Reversed Z falls as the inverse of distance, so scaling it brings the point nearer by
    // about that share of its distance.
    let z = min(depth * (1.0 + viewport.depth_bias), 1.0);
    return Varyings(vec4<f32>(ndc, z, 1.0), color);
}

@fragment
fn fs(in: Varyings) -> @location(0) vec4<f32> {
    return in.color;
}
