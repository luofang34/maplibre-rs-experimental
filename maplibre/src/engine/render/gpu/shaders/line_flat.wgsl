// @include line_ground.wgsl

// A line on the map plane, or at an elevation of its own: no terrain lies beneath it.
fn line_ground(position: vec2<f32>, tile_mercator_coords: vec4<f32>) -> LineGround {
    return LineGround(false, 0.0, vec2<f32>(0.0), vec2<f32>(2048.0));
}
