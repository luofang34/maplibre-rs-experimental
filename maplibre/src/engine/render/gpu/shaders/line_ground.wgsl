// Where a vertex lies on the terrain surface, from the `line_ground` of the pipeline's prelude.
struct LineGround {
    // Whether the line lies on the terrain surface rather than on the map plane.
    on_terrain: bool,
    // Height of the surface in metres.
    elevation: f32,
    // Slope of the surface in metres per tile unit of the line's tile, along x and y.
    slope: vec2<f32>,
    // Position in the bound terrain tile, in tile units.
    terrain_position: vec2<f32>,
};
