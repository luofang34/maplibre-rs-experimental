struct VertexOutput {
    @location(0) color: vec4<f32>,
    @location(1) @interpolate(flat) horizon: vec4<f32>,
    @location(2) @interpolate(flat) viewport: vec4<f32>,
    @location(3) @interpolate(flat) row_numerator: vec4<f32>,
    @location(4) @interpolate(flat) row_denominator: vec4<f32>,
    @builtin(position) position: vec4<f32>,
};

@vertex
fn main(
    @builtin(vertex_index) vertex_idx: u32,
    @location(0) color: vec4<f32>,
    @location(1) z_index: f32, // Passed from per-layer metadata
    @location(2) horizon: vec4<f32>,
    @location(3) viewport: vec4<f32>,
    @location(4) row_numerator: vec4<f32>,
    @location(5) row_denominator: vec4<f32>,
) -> VertexOutput {
    // Generate a fullscreen quad using standard 6-vertex triangle list layout
    var positions = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>( 1.0,  1.0)
    );

    let pos = positions[vertex_idx % 6u];
    
    // Output raw clip space coordinates (identity mapping)
    var out: VertexOutput;
    
    // We use a small epsilon near 0.0 (the far plane) because wgpu `Greater` won't pass 0.0 > 0.0 
    out.position = vec4<f32>(pos, 1.0e-5, 1.0);
    out.color = color;
    out.horizon = horizon;
    out.viewport = viewport;
    out.row_numerator = row_numerator;
    out.row_denominator = row_denominator;

    return out;
}
