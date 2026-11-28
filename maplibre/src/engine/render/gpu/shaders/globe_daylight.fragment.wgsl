// @include daylight.wgsl.part

// The share of its colour each pixel of the globe keeps under the host's sun, which the blend
// multiplies into what the main pass drew. It follows `daylight_weights` as terrain does, but
// as a factor it tints the night side without desaturating it.

struct VertexInput {
    @location(0) view_direction: vec3<f32>,
    @location(1) @interpolate(flat) globe: vec4<f32>,
    @location(2) @interpolate(flat) sun: vec3<f32>,
    @location(3) @interpolate(flat) twilight: vec4<f32>,
    @location(4) @interpolate(flat) moonlight: vec4<f32>,
};

// The globe's normal where the view ray meets it, in view axes. `globe` holds the globe's centre
// in view axes and its radius, in the same units. Near an eye standing on the globe the ray's
// discriminant cancels to rounding, so a ray that seems to miss takes the point where it passes
// closest to the centre: its normal differs by far less than the twilight band is wide.
fn surface_normal(direction: vec3<f32>, globe: vec4<f32>) -> vec3<f32> {
    let origin = -globe.xyz / globe.w;
    let ray = normalize(direction);
    let b = dot(origin, ray);
    let discriminant = b * b - (dot(origin, origin) - 1.0);
    let root = sqrt(max(discriminant, 0.0));
    let near = -b - root;
    let t = select(-b + root, near, near > 0.0);
    return normalize(origin + ray * select(max(-b, 0.0), t, discriminant >= 0.0));
}

@fragment
fn main(in: VertexInput) -> @location(0) vec4<f32> {
    let normal = surface_normal(in.view_direction, in.globe);
    let weights = daylight_weights(normal, in.sun, in.twilight, in.moonlight);
    return vec4<f32>(mix(NIGHT_TINT * weights.y, vec3<f32>(1.0), weights.x), 1.0);
}
