struct PatternUniforms {
    // Width and height of the pattern image in layout pixels.
    size: vec4<f32>,
};

// Where the screen looks at the map, and how to get back from a screen position to it.
struct ViewUniforms {
    // Clip position to map pixels relative to the centre of the view.
    clip_to_map: mat4x4<f32>,
    // The centre of the view in map pixels, split so that the pattern size can be removed
    // from it exactly: high part in xy, low part in zw.
    center: vec4<f32>,
    // Width and height of the viewport in pixels.
    viewport: vec4<f32>,
};

@group(0) @binding(0) var<uniform> pattern: PatternUniforms;
@group(0) @binding(1) var pattern_texture: texture_2d<f32>;
@group(0) @binding(2) var pattern_sampler: sampler;
@group(1) @binding(0) var<uniform> view: ViewUniforms;

struct Output {
    @location(0) out_color: vec4<f32>,
};

@fragment
fn main(
    @location(0) v_color: vec4<f32>,
    @location(1) @interpolate(flat) horizon: vec4<f32>,
    @location(2) @interpolate(flat) viewport: vec4<f32>,
    @builtin(position) position: vec4<f32>,
) -> Output {
    let y = viewport.x - position.y;
    let distance = (y - horizon.y) * horizon.w + (position.x - horizon.x) * horizon.z;
    if distance > 0.0 {
        discard;
    }
    let ndc = vec2<f32>(
        position.x / view.viewport.x * 2.0 - 1.0,
        1.0 - position.y / view.viewport.y * 2.0,
    );
    let near = view.clip_to_map * vec4<f32>(ndc, 0.0, 1.0);
    let far = view.clip_to_map * vec4<f32>(ndc, 1.0, 1.0);
    let ray_start = near.xyz / near.w;
    let ray_end = far.xyz / far.w;
    let hit = -ray_start.z / (ray_end.z - ray_start.z);
    let delta = (ray_start + hit * (ray_end - ray_start)).xy;

    let size = pattern.size.xy;
    let upper = view.center.xy - size * floor(view.center.xy / size);
    let coarse = upper * 256.0 - size * floor(upper * 256.0 / size);
    let low = coarse * 256.0 + view.center.zw;
    let offset = low - size * floor(low / size);
    let uv = fract((delta + offset) / size);
    let texel = textureSample(pattern_texture, pattern_sampler, uv);
    // The texture holds premultiplied colours, so filtering does not fringe the edges.
    return Output(texel * v_color.a);
}
