struct FragmentInput {
    @location(0) extrude: vec2<f32>,
    @location(1) strength: f32,
    @location(2) horizon_distance: f32,
};

// The Gaussian's normalisation, 1 / sqrt(2 * pi), as the GL JS kernel uses it.
const GAUSS_COEF: f32 = 0.3989422804014327;

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
    if in.horizon_distance < 0.0 {
        discard;
    }
    let distance_squared = dot(in.extrude, in.extrude);
    // Three standard deviations across the radius.
    let density = in.strength * GAUSS_COEF * exp(-0.5 * 3.0 * 3.0 * distance_squared);
    return vec4<f32>(density, 0.0, 0.0, 1.0);
}
