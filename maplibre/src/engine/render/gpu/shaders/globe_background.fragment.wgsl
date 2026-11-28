struct Output {
    @location(0) out_color: vec4<f32>,
};

@fragment
fn main(
    @location(0) color: vec4<f32>,
) -> Output {
    return Output(vec4<f32>(color.rgb * color.a, color.a));
}
