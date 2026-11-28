@vertex
fn main(@builtin(vertex_index) idx: u32) -> @builtin(position) vec4<f32> {
    let x = select(-1.0, 3.0, idx == 1u);
    let y = select(-1.0, 3.0, idx == 2u);
    return vec4<f32>(x, y, 0.0, 1.0);
}
