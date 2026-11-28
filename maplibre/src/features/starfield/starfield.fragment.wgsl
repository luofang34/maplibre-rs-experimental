struct VertexInput {
    @location(0) corner: vec2<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
};

@fragment
fn main(in: VertexInput) -> @location(0) vec4<f32> {
    // A round point that fades to its edge, so faint stars do not alias into squares.
    let alpha = in.color.a * (1.0 - smoothstep(0.3, 1.0, length(in.corner)));
    return vec4<f32>(in.color.rgb * alpha, alpha);
}
