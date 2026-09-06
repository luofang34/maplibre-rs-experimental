@group(0) @binding(0) var source: texture_depth_2d;

struct Output {
    @builtin(frag_depth) depth: f32,
}

@fragment
fn main(@builtin(position) position: vec4<f32>) -> Output {
    var output: Output;
    output.depth = textureLoad(source, vec2<i32>(position.xy), 0);
    return output;
}
