struct VertexOutput {
    @location(0) tex_coords: vec3<f32>,
    @location(1) horizon_distance: f32,
    @location(2) mercator_y: f32,
    @location(3) zoom: f32,
    @builtin(position) position: vec4<f32>,
};

@group(1) @binding(0)
var t_dem: texture_2d<f32>;
@group(1) @binding(1)
var s_dem: sampler;

struct ColorReliefUniforms {
    unpack: vec4<f32>,
    opacity: f32,
    stop_count: u32,
    padding: vec2<u32>,
    // Stop elevations, four per vector.
    elevations: array<vec4<f32>, 16>,
    // Premultiplied stop colours.
    colors: array<vec4<f32>, 64>,
};

@group(2) @binding(0)
var<uniform> relief: ColorReliefUniforms;

fn stop_elevation(stop: u32) -> f32 {
    return relief.elevations[stop / 4u][stop % 4u];
}

@fragment
fn main(in: VertexOutput) -> @location(0) vec4<f32> {
    if in.horizon_distance < 0.0 {
        discard;
    }
    let uv = in.tex_coords.xy / in.tex_coords.z;
    let data = textureSample(t_dem, s_dem, uv) * 255.0;
    let elevation = dot(vec4<f32>(data.rgb, -1.0), relief.unpack);
    let count = relief.stop_count;
    if count == 0u {
        discard;
    }
    if count == 1u {
        return relief.colors[0] * relief.opacity;
    }
    // Binary search for the stops around the elevation.
    var low = 0u;
    var high = count - 1u;
    while high - low > 1u {
        let middle = (low + high) / 2u;
        if elevation < stop_elevation(middle) {
            high = middle;
        } else {
            low = middle;
        }
    }
    let span = stop_elevation(high) - stop_elevation(low);
    var t = 0.0;
    if span > 0.0 {
        t = clamp((elevation - stop_elevation(low)) / span, 0.0, 1.0);
    }
    return mix(relief.colors[low], relief.colors[high], t) * relief.opacity;
}
