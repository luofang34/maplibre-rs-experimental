@group(0) @binding(0) var source: texture_depth_multisampled_2d;

struct Output {
    @builtin(frag_depth) depth: f32,
}

// Depth is reversed, so the largest sample is the nearest surface, which is what a
// compositor occluding the frame against the world should see.
@fragment
fn main(@builtin(position) position: vec4<f32>) -> Output {
    let coordinates = vec2<i32>(position.xy);
    let samples = i32(textureNumSamples(source));
    var depth = 0.0;
    for (var sample = 0; sample < samples; sample++) {
        depth = max(depth, textureLoad(source, coordinates, sample));
    }
    var output: Output;
    output.depth = depth;
    return output;
}
