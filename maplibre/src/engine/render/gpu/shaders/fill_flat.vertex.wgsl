struct VertexOutput {
    @location(0) v_color: vec4<f32>,
    @builtin(position) position: vec4<f32>,
};

@vertex
fn main(
    @location(0) position: vec2<f32>,
    @location(1) normal: vec2<f32>,
    @location(4) translate1: vec4<f32>,
    @location(5) translate2: vec4<f32>,
    @location(6) translate3: vec4<f32>,
    @location(7) translate4: vec4<f32>,
    @location(8) color: vec4<f32>,
    @location(9) zoom_factor: f32,
    @location(15) layer_translate: vec2<f32>,
) -> VertexOutput {
    let pos = position + layer_translate + normal * (3.0 * zoom_factor);
    let is_pole = abs(pos.y + 32768.0) < 0.5 || abs(pos.y - 32767.0) < 0.5;
    let clip = mat4x4<f32>(translate1, translate2, translate3, translate4) * vec4<f32>(pos, 0.0, 1.0);
    return VertexOutput(color, vec4<f32>(clip.xy, select(0.5, -0.5, is_pole) * clip.w, clip.w));
}
