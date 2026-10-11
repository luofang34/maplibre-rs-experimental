// Host overlay triangles: positions in render-target pixels, premultiplied colours.

struct Viewport {
    size: vec2<f32>,
};

@group(0) @binding(0) var<uniform> viewport: Viewport;

struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs(@location(0) position: vec2<f32>, @location(1) color: vec4<f32>) -> Varyings {
    let ndc = vec2<f32>(position.x / viewport.size.x * 2.0 - 1.0, 1.0 - position.y / viewport.size.y * 2.0);
    return Varyings(vec4<f32>(ndc, 0.0, 1.0), color);
}

@fragment
fn fs(in: Varyings) -> @location(0) vec4<f32> {
    return in.color;
}
