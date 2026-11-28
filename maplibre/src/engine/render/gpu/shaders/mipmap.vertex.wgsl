struct VertexOutput {
    @location(0) uv: vec2<f32>,
    @builtin(position) position: vec4<f32>,
};

// One triangle covering the whole level, its texture coordinates spanning 0..1 across it.
@vertex
fn main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    let uv = vec2<f32>(f32((vertex_index << 1u) & 2u), f32(vertex_index & 2u));
    return VertexOutput(uv, vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0));
}
