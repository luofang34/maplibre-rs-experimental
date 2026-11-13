#![allow(clippy::expect_used, clippy::panic)]
use lyon::{math::point, path::Path, tessellation::VertexBuffers};

use super::{
    clip::rings, clipped, extrude, next_zoom_code, pack_extrusion_code, unpack_datum_pair,
    ExtrusionHeights, FLOOR_NORMAL,
};
use crate::{
    render::ShaderVertex,
    style::layer::{ElevationDatum, ExtrusionPart},
};

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
            ..ExtrusionHeights::constant(1.0, 10.0)
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

#[test]
fn translucent_floating_volume_emits_outward_wound_roof_floor_and_walls() {
    let mut builder = Path::builder();
    square(&mut builder, [100.0, 100.0, 300.0, 300.0]);
    let mut buffer: VertexBuffers<ShaderVertex, u32> = VertexBuffers::new();
    let mut heights = ExtrusionHeights::constant(500.0, 1500.0);
    heights.translucent = true;
    heights.suppress_tile_boundary_walls = true;
    heights.base_datum = ElevationDatum::Msl;
    heights.height_datum = ElevationDatum::Msl;
    extrude(
        &builder.build(),
        heights,
        &mut buffer,
        (0.02, |_: &mut VertexBuffers<ShaderVertex, u32>, _| Ok(())),
    )
    .expect("extrudes volume");

    let roof_count = buffer
        .vertices
        .iter()
        .filter(|v| v.normal == [0.0, 0.0])
        .count();
    let floor_count = buffer
        .vertices
        .iter()
        .filter(|v| v.normal == FLOOR_NORMAL)
        .count();
    let wall_count = buffer
        .vertices
        .iter()
        .filter(|v| v.normal[0].hypot(v.normal[1]) > 0.5)
        .count();
    assert_eq!(roof_count, 4);
    assert_eq!(floor_count, 4);
    assert_eq!(wall_count, 16);

    // Every triangle's 3D outward normal in right-handed (East=+x, North=-y, Up=+z) must point
    // away from the box's interior centroid `(200, -200, 1000)`.
    let center = [200.0_f32, -200.0, 1000.0];
    let pos3 = |v: &ShaderVertex| {
        let len = v.normal[0].hypot(v.normal[1]);
        let top = len == 0.0 || len > 1.5;
        let z = if top { v.elevation } else { v.distance };
        [v.position[0], -v.position[1], z]
    };
    for tri in buffer.indices.as_chunks::<3>().0 {
        let a = pos3(&buffer.vertices[tri[0] as usize]);
        let b = pos3(&buffer.vertices[tri[1] as usize]);
        let c = pos3(&buffer.vertices[tri[2] as usize]);
        let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let normal = [
            ab[1] * ac[2] - ab[2] * ac[1],
            ab[2] * ac[0] - ab[0] * ac[2],
            ab[0] * ac[1] - ab[1] * ac[0],
        ];
        let outward = [a[0] - center[0], a[1] - center[1], a[2] - center[2]];
        let dot = normal[0] * outward[0] + normal[1] * outward[1] + normal[2] * outward[2];
        assert!(dot > 0.0, "triangle {tri:?} wound inward: dot={dot}");
    }
}

#[test]
fn surface_translucent_volume_omits_floor_cap_and_tile_seam_walls() {
    let mut builder = Path::builder();
    square(&mut builder, [3000.0, 100.0, 4500.0, 300.0]);
    let clipped_path = clipped(&builder.build(), 0.0);
    let mut buffer: VertexBuffers<ShaderVertex, u32> = VertexBuffers::new();
    let mut heights = ExtrusionHeights::constant(0.0, 1000.0);
    heights.translucent = true;
    heights.suppress_tile_boundary_walls = true;
    extrude(
        &clipped_path,
        heights,
        &mut buffer,
        (0.02, |_: &mut VertexBuffers<ShaderVertex, u32>, _| Ok(())),
    )
    .expect("extrudes");
    assert_eq!(
        buffer
            .vertices
            .iter()
            .filter(|v| v.normal == FLOOR_NORMAL)
            .count(),
        0
    );
    // Only 3 interior walls (4 * 3 = 12 vertices); the wall on x = 4096 is suppressed.
    let wall_vertices = buffer
        .vertices
        .iter()
        .filter(|v| v.normal[0].hypot(v.normal[1]) > 0.5)
        .count();
    assert_eq!(wall_vertices, 12);
}

#[test]
fn datum_pairs_roundtrip_without_corrupting_zoom_rise() {
    for base in [
        ElevationDatum::CentroidAgl,
        ElevationDatum::Msl,
        ElevationDatum::VertexAgl,
    ] {
        for height in [
            ElevationDatum::CentroidAgl,
            ElevationDatum::Msl,
            ElevationDatum::VertexAgl,
        ] {
            let pair = ElevationDatum::pack_pair(base, height);
            let code = pack_extrusion_code(-250.0, 4, pair);
            assert_eq!(unpack_datum_pair(code), (base, height));
            let zoom_code = code - f32::from(pair) * super::DATUM_CODE_SPAN;
            let overscale = (zoom_code / super::NEXT_ZOOM_CODE_SPAN).floor();
            let rise = zoom_code
                - overscale * super::NEXT_ZOOM_CODE_SPAN
                - 0.5 * super::NEXT_ZOOM_CODE_SPAN;
            assert_eq!((overscale, rise), (4.0, -250.0));
        }
    }
}

#[test]
fn extrusion_part_selectors_emit_only_requested_shell_components() {
    let mut builder = Path::builder();
    square(&mut builder, [100.0, 100.0, 300.0, 300.0]);
    let path = builder.build();
    let counts = |part: ExtrusionPart| {
        let mut buffer: VertexBuffers<ShaderVertex, u32> = VertexBuffers::new();
        let mut heights = ExtrusionHeights::constant(500.0, 1500.0);
        heights.translucent = true;
        heights.part = part;
        extrude(
            &path,
            heights,
            &mut buffer,
            (0.02, |_: &mut VertexBuffers<ShaderVertex, u32>, _| Ok(())),
        )
        .expect("extrudes part");
        let roofs = buffer
            .vertices
            .iter()
            .filter(|v| v.normal == [0.0, 0.0])
            .count();
        let floors = buffer
            .vertices
            .iter()
            .filter(|v| v.normal == FLOOR_NORMAL)
            .count();
        let wall_verts = buffer
            .vertices
            .iter()
            .filter(|v| v.normal[0].hypot(v.normal[1]) > 0.5)
            .count();
        (roofs, floors, wall_verts, buffer)
    };

    assert_eq!(
        {
            let c = counts(ExtrusionPart::Walls);
            (c.0, c.1, c.2)
        },
        (0, 0, 16)
    );
    assert_eq!(
        {
            let c = counts(ExtrusionPart::Roof);
            (c.0, c.1, c.2)
        },
        (4, 0, 0)
    );
    let (roofs, floors, wall_verts, floor_buf) = counts(ExtrusionPart::Floor);
    assert_eq!((roofs, floors, wall_verts), (0, 4, 0));
    for tri in floor_buf.indices.as_chunks::<3>().0 {
        let p0 = floor_buf.vertices[tri[0] as usize].position;
        let p1 = floor_buf.vertices[tri[1] as usize].position;
        let p2 = floor_buf.vertices[tri[2] as usize].position;
        let cross_xy = (p1[0] - p0[0]) * (p2[1] - p0[1]) - (p1[1] - p0[1]) * (p2[0] - p0[0]);
        // Floor triangles must wind `-Up` (`cross_xy > 0` in tile y-down coordinates).
        assert!(
            cross_xy > 0.0,
            "floor cap triangle wound upward: {cross_xy}"
        );
    }
}

#[test]
fn subdivided_vertex_agl_walls_preserve_continuous_edge_distance_across_seams() {
    let mut builder = Path::builder();
    square(&mut builder, [100.0, 100.0, 612.0, 356.0]);
    let path = builder.build();

    let build = |datum: ElevationDatum| {
        let mut buffer: VertexBuffers<ShaderVertex, u32> = VertexBuffers::new();
        let mut heights = ExtrusionHeights::constant(0.0, 80.0);
        heights.part = ExtrusionPart::Walls;
        heights.height_datum = datum;
        extrude(
            &path,
            heights,
            &mut buffer,
            (0.02, |_: &mut VertexBuffers<ShaderVertex, u32>, _| Ok(())),
        )
        .expect("extrudes walls");
        buffer
    };

    let unsubdivided = build(ElevationDatum::CentroidAgl);
    let subdivided = build(ElevationDatum::VertexAgl);
    // 512-unit horizontal walls split into 4 quads each; 256-unit vertical walls into 2 quads each.
    assert_eq!(unsubdivided.vertices.len(), 16);
    assert_eq!(subdivided.vertices.len(), (4 + 2 + 4 + 2) * 4);

    // Every subdivided quad (`4` vertices: `[a_bot, a_top, b_top, b_bot]`) along the first 512-unit
    // edge `(100,100) -> (612,100)` must interpolate smoothly from `512.0` down to `0.0` with exact
    // seam continuity between adjacent quads (`prev.b == next.a`).
    let first_wall = &subdivided.vertices[..16];
    assert_eq!(
        first_wall[0].edge_distance,
        unsubdivided.vertices[0].edge_distance
    );
    assert_eq!(
        first_wall[14].edge_distance,
        unsubdivided.vertices[2].edge_distance
    );
    for quad_pair in first_wall.as_chunks::<4>().0.windows(2) {
        let prev_end = quad_pair[0][2];
        let next_start = quad_pair[1][1];
        assert_eq!(prev_end.position, next_start.position);
        assert_eq!(
            prev_end.edge_distance, next_start.edge_distance,
            "subdivided wall seam must preserve continuous fill-extrusion-pattern edge_distance"
        );
    }
}
