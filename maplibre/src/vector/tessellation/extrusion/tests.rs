#![allow(clippy::expect_used, clippy::panic)]
use lyon::{math::point, path::Path, tessellation::VertexBuffers};

use super::{clipped, extrude, next_zoom_code, rings, ExtrusionHeights};
use crate::render::ShaderVertex;

fn square(builder: &mut lyon::path::path::Builder, [x0, y0, x1, y1]: [f32; 4]) {
    builder.begin(point(x0, y0));
    builder.line_to(point(x1, y0));
    builder.line_to(point(x1, y1));
    builder.line_to(point(x0, y1));
    builder.end(true);
}

fn extruded(squares: &[[f32; 4]]) -> VertexBuffers<ShaderVertex, u32> {
    let mut builder = Path::builder();
    for corners in squares {
        square(&mut builder, *corners);
    }
    let mut buffer = VertexBuffers::new();
    extrude(
        &builder.build(),
        ExtrusionHeights::constant(2.0, 10.0),
        &mut buffer,
        (0.02, |_: &mut VertexBuffers<ShaderVertex, u32>, _| Ok(())),
    )
    .expect("extrudes");
    buffer
}

fn walls(buffer: &VertexBuffers<ShaderVertex, u32>) -> Vec<&ShaderVertex> {
    buffer
        .vertices
        .iter()
        .filter(|vertex| vertex.normal != [0.0, 0.0])
        .collect()
}

#[test]
fn a_square_has_a_roof_and_four_walls_between_base_and_height() {
    let buffer = extruded(&[[0.0, 0.0, 100.0, 100.0]]);
    assert_eq!(walls(&buffer).len(), 16);
    let roof: Vec<_> = buffer
        .vertices
        .iter()
        .filter(|vertex| vertex.normal == [0.0, 0.0])
        .collect();
    assert_eq!(roof.len(), 4);
    assert!(buffer
        .vertices
        .iter()
        .all(|vertex| vertex.distance == 2.0 && vertex.elevation == 10.0));
}

#[test]
fn the_top_edge_of_a_wall_carries_a_normal_twice_as_long_as_the_bottom_edge() {
    let buffer = extruded(&[[0.0, 0.0, 100.0, 100.0]]);
    let lengths: Vec<f32> = walls(&buffer)
        .iter()
        .map(|vertex| vertex.normal[0].hypot(vertex.normal[1]))
        .collect();
    assert_eq!(
        lengths
            .iter()
            .filter(|length| (**length - 1.0).abs() < 1e-6)
            .count(),
        8
    );
    assert_eq!(
        lengths
            .iter()
            .filter(|length| (**length - 2.0).abs() < 1e-6)
            .count(),
        8
    );
}

#[test]
fn a_hole_gets_walls_facing_the_other_way() {
    let buffer = extruded(&[[0.0, 0.0, 100.0, 100.0], [40.0, 40.0, 60.0, 60.0]]);
    assert_eq!(walls(&buffer).len(), 32);
    let side = |x: f32, y: f32| {
        buffer
            .vertices
            .iter()
            .find(|vertex| vertex.normal != [0.0, 0.0] && vertex.position == [x, y])
            .expect("wall vertex")
            .normal[0]
            .signum()
    };
    // The outer wall on the left edge and the hole's wall on its left edge face opposite ways.
    assert_ne!(side(0.0, 0.0), side(40.0, 40.0));
}

#[test]
fn walls_along_the_clipped_edge_of_a_tile_are_left_out() {
    let buffer = extruded(&[[-200.0, 0.0, -100.0, 100.0]]);
    assert_eq!(walls(&buffer).len(), 8);
}

#[test]
fn clipping_keeps_a_polygon_inside_the_buffer() {
    use lyon::math::point;
    let mut builder = Path::builder();
    builder.begin(point(3201.0, 3201.0));
    for corner in [
        (3201.0, 4991.0),
        (4991.0, 4991.0),
        (4991.0, 3201.0),
        (3201.0, 3201.0),
    ] {
        builder.line_to(point(corner.0, corner.1));
    }
    builder.end(true);
    let clipped = clipped(&builder.build(), 1024.0);
    let ring = &rings(&clipped)[0];
    assert_eq!(ring.len(), 5, "{ring:?}");
}

/// Every vertex carries how much it rises one zoom up, the roof and the top of a wall by the
/// roof's rise and the bottom of a wall by the base's, with the tile's overscale.
#[test]
fn vertices_carry_their_rise_one_zoom_up() {
    let mut builder = Path::builder();
    square(&mut builder, [0.0, 0.0, 100.0, 100.0]);
    let mut buffer: VertexBuffers<ShaderVertex, u32> = VertexBuffers::new();
    extrude(
        &builder.build(),
        ExtrusionHeights {
            base: 1.0,
            height: 10.0,
            base_next: 3.0,
            height_next: 40.0,
            overscale: 1,
        },
        &mut buffer,
        (0.02, |_: &mut VertexBuffers<ShaderVertex, u32>, _| Ok(())),
    )
    .expect("extrudes");

    for vertex in &buffer.vertices {
        let length = vertex.normal[0].hypot(vertex.normal[1]);
        let top = length == 0.0 || length > 1.5;
        let expected = if top { 30.0 } else { 2.0 };
        assert_eq!(vertex.corner_code, next_zoom_code(expected, 1));
    }
}

/// The shader's decoding of the code, with a rise that is negative as when a building's height
/// ramps down with zoom.
#[test]
fn the_code_keeps_rise_and_overscale_apart() {
    let span = super::NEXT_ZOOM_CODE_SPAN;
    let decode = |code: f32| {
        let overscale = (code / span).floor();
        (overscale, code - overscale * span - span / 2.0)
    };
    assert_eq!(decode(next_zoom_code(123.25, 3)), (3.0, 123.25));
    assert_eq!(decode(next_zoom_code(-40.5, 0)), (0.0, -40.5));
    assert_eq!(decode(next_zoom_code(0.0, 2)), (2.0, 0.0));
    // Rises out of range are kept inside the span so they cannot change the overscale.
    assert_eq!(decode(next_zoom_code(1.0e9, 1)).0, 1.0);
    assert_eq!(decode(next_zoom_code(-1.0e9, 1)).0, 1.0);
}
