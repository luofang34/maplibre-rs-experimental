#![allow(clippy::expect_used, clippy::panic)]
use lyon::{math::point, path::Path, tessellation::VertexBuffers};

use super::extrude;
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
        (2.0, 10.0),
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
