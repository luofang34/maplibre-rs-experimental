use lyon::tessellation::{VertexBuffers, VertexId};

use super::{
    granularity_for_zoom, subdivide_line_segment, subdivide_triangles, FillSubdivisionOptions,
};
use crate::render::ShaderVertex;

#[test]
fn granularity_matches_gl_js_globe_policy() {
    assert_eq!(granularity_for_zoom(128, 2, 0), 128);
    assert_eq!(granularity_for_zoom(128, 2, 6), 2);
    assert_eq!(granularity_for_zoom(128, 2, 31), 2);
    assert_eq!(granularity_for_zoom(512, 0, 0), 512);
    assert_eq!(granularity_for_zoom(512, 0, 9), 1);
}

#[test]
fn line_subdivision_matches_reference_examples() {
    let horizontal = subdivide_line_segment([0.0, 0.0], [4096.0, 0.0], 4)
        .expect("reference line should subdivide");
    assert_eq!(
        horizontal,
        vec![[1024.0, 0.0], [2048.0, 0.0], [3072.0, 0.0], [4096.0, 0.0]]
    );

    let diagonal = subdivide_line_segment([4096.0, 0.0], [0.0, 4096.0], 4)
        .expect("reference diagonal should subdivide");
    assert_eq!(
        diagonal,
        vec![
            [3072.0, 1024.0],
            [2048.0, 2048.0],
            [1024.0, 3072.0],
            [0.0, 4096.0]
        ]
    );
}

#[test]
fn fill_triangle_is_cut_on_both_grid_axes() {
    let mut buffer = VertexBuffers::<ShaderVertex, u32> {
        vertices: vec![
            ShaderVertex::new([0.0, 0.0], [0.0; 2]),
            ShaderVertex::new([4096.0, 0.0], [0.0; 2]),
            ShaderVertex::new([0.0, 4096.0], [0.0; 2]),
        ],
        indices: vec![0, 1, 2],
    };

    subdivide_triangles(&mut buffer, 0, options(2)).expect("reference triangle should subdivide");

    assert!(buffer.indices.len() > 3);
    for index in &buffer.indices {
        assert!((*index as usize) < buffer.vertices.len());
    }
    assert!(buffer
        .vertices
        .iter()
        .any(|vertex| vertex.position == [2048.0, 0.0]));
    assert!(buffer
        .vertices
        .iter()
        .any(|vertex| vertex.position == [0.0, 2048.0]));
    let _: VertexId = 0_u32.into();
}

#[test]
fn zoom_zero_clipping_drops_buffered_antimeridian_geometry() {
    let mut buffer = VertexBuffers::<ShaderVertex, u32> {
        vertices: vec![
            ShaderVertex::new([-10.0, 0.0], [0.0; 2]),
            ShaderVertex::new([10.0, 0.0], [0.0; 2]),
            ShaderVertex::new([10.0, 10.0], [0.0; 2]),
        ],
        indices: vec![0, 1, 2],
    };

    let mut subdivision = options(2);
    subdivision.clip_x_to_tile = true;
    subdivide_triangles(&mut buffer, 0, subdivision).expect("clipped triangle should be valid");

    assert!(!buffer.indices.is_empty());
    assert!(buffer.indices.iter().all(|index| {
        let x = buffer.vertices[*index as usize].position[0];
        (0.0..=4096.0).contains(&x)
    }));
}

#[test]
fn north_edge_generates_special_pole_vertices() {
    let mut buffer = VertexBuffers::<ShaderVertex, u32> {
        vertices: vec![
            ShaderVertex::new([0.0, 0.0], [0.0; 2]),
            ShaderVertex::new([4096.0, 0.0], [0.0; 2]),
            ShaderVertex::new([0.0, 4096.0], [0.0; 2]),
        ],
        indices: vec![0, 1, 2],
    };
    let mut subdivision = options(2);
    subdivision.extend_to_north_pole = true;

    subdivide_triangles(&mut buffer, 0, subdivision).expect("pole fill should subdivide");

    assert!(buffer
        .vertices
        .iter()
        .any(|vertex| vertex.position[1] == i16::MIN as f32));
}

fn options(granularity: u32) -> FillSubdivisionOptions {
    FillSubdivisionOptions {
        granularity,
        clip_x_to_tile: false,
        extend_to_north_pole: false,
        extend_to_south_pole: false,
    }
}

#[test]
fn subdivision_preserves_fractional_road_alignment() {
    let start = [2046.8622, 2047.5905];
    let end = [2049.1377, 2047.5905];
    let points = subdivide_line_segment(start, end, 512).expect("subdivide road");
    assert!(points.len() > 1);
    assert!(points.iter().all(|point| point[1] == start[1]));
}

#[test]
fn inline_polygon_clipping_ping_pong_produces_clipped_quad() {
    use super::{clip_axis_into, InlinePolygon};

    let triangle = [[0.0, 0.0], [4096.0, 0.0], [0.0, 4096.0]];
    let poly_a = InlinePolygon::from_triangle(triangle);
    let mut poly_b = InlinePolygon::empty();
    let mut poly_c = InlinePolygon::empty();

    // Clip x >= 1024
    clip_axis_into(&poly_a, &mut poly_b, 0, 1024.0, true);
    assert_eq!(poly_b.len(), 3);

    // Clip x <= 2048
    clip_axis_into(&poly_b, &mut poly_c, 0, 2048.0, false);
    assert_eq!(poly_c.len(), 4);

    // Ping pong back to poly_b: Clip y >= 512
    clip_axis_into(&poly_c, &mut poly_b, 1, 512.0, true);
    assert!(poly_b.len() >= 3);
}

#[test]
fn inline_polygon_capacity_guard_and_clearing() {
    use super::{InlinePolygon, MAX_CLIPPED_POLYGON_VERTICES};

    let mut poly = InlinePolygon::empty();
    assert_eq!(poly.len(), 0);

    for i in 0..MAX_CLIPPED_POLYGON_VERTICES + 5 {
        poly.push([i as f32, i as f32]);
    }
    assert_eq!(poly.len(), MAX_CLIPPED_POLYGON_VERTICES);

    poly.clear();
    assert_eq!(poly.len(), 0);
}

fn pole_vertex_indices(buffer: &VertexBuffers<ShaderVertex, u32>) -> Vec<(usize, f32)> {
    buffer
        .vertices
        .iter()
        .enumerate()
        .filter(|(_, vertex)| {
            vertex.position[1] == i16::MIN as f32 || vertex.position[1] == i16::MAX as f32
        })
        .map(|(index, vertex)| (index, vertex.position[1]))
        .collect()
}

/// Appends one polygon's triangles to a buffer shared with earlier ones and subdivides them, as
/// the tessellator does for each feature of a layer.
fn subdivide_appended(
    buffer: &mut VertexBuffers<ShaderVertex, u32>,
    triangles: &[[[f32; 2]; 3]],
    subdivision: FillSubdivisionOptions,
) {
    let index_start = buffer.indices.len();
    for triangle in triangles {
        for point in triangle {
            buffer.indices.push(buffer.vertices.len() as u32);
            buffer.vertices.push(ShaderVertex::new(*point, [0.0; 2]));
        }
    }
    subdivide_triangles(buffer, index_start, subdivision).expect("polygon should subdivide");
}

#[test]
fn later_polygons_leave_earlier_pole_vertices_exact() {
    let mut subdivision = options(2);
    subdivision.extend_to_north_pole = true;
    subdivision.extend_to_south_pole = true;
    let mut buffer = VertexBuffers::<ShaderVertex, u32>::new();
    let whole_tile = [
        [[0.0, 0.0], [4096.0, 0.0], [4096.0, 4096.0]],
        [[0.0, 0.0], [4096.0, 4096.0], [0.0, 4096.0]],
    ];
    subdivide_appended(&mut buffer, &whole_tile, subdivision);
    let poles = pole_vertex_indices(&buffer);
    assert!(poles.iter().any(|(_, y)| *y == i16::MIN as f32));
    assert!(poles.iter().any(|(_, y)| *y == i16::MAX as f32));

    let lake = [[[1000.0, 1000.0], [1500.0, 1000.0], [1000.0, 1500.0]]];
    subdivide_appended(&mut buffer, &lake, subdivision);

    for (index, y) in &poles {
        assert_eq!(
            buffer.vertices[*index].position[1], *y,
            "pole vertex {index}"
        );
    }
}

#[test]
fn input_on_a_pole_sentinel_never_becomes_a_pole_vertex() {
    let mut buffer = VertexBuffers::<ShaderVertex, u32>::new();
    let reaching_past_the_tile = [[[0.0, -32_767.7], [4096.0, 32_767.0], [0.0, 32_766.6]]];
    subdivide_appended(&mut buffer, &reaching_past_the_tile, options(2));

    assert!(buffer.indices.iter().all(|index| {
        let y = buffer.vertices[*index as usize].position[1];
        y != i16::MIN as f32 && y != i16::MAX as f32
    }));
}

#[test]
fn buffered_polar_polygon_extrudes_once_from_outer_buffer_edge_without_double_overlap_at_extent() {
    let mut subdivision = options(2);
    subdivision.extend_to_south_pole = true;
    let mut buffer = VertexBuffers::<ShaderVertex, u32>::new();
    // Buffered Antarctica tile polygon spanning y = 3000..4176 across y = 4096.
    let buffered_antarctica = [
        [[0.0, 3000.0], [4096.0, 3000.0], [4096.0, 4176.0]],
        [[0.0, 3000.0], [4096.0, 4176.0], [0.0, 4176.0]],
    ];
    subdivide_appended(&mut buffer, &buffered_antarctica, subdivision);

    let south_pole = i16::MAX as f32;
    let mut pole_triangles = 0;
    for tri in buffer.indices.as_chunks::<3>().0 {
        let ys = [
            buffer.vertices[tri[0] as usize].position[1],
            buffer.vertices[tri[1] as usize].position[1],
            buffer.vertices[tri[2] as usize].position[1],
        ];
        if ys.contains(&south_pole) {
            pole_triangles += 1;
            // Every pole quad triangle must attach to the true buffered clip edge y = 4176,
            // never to the interior y = 4096 subdivision line (which caused double-alpha overlap).
            assert!(
                ys.iter().all(|&y| y == south_pole || y == 4176.0),
                "pole triangle attached to interior y instead of buffer clip: {ys:?}"
            );
        }
    }
    // At granularity=2 (cell_size=2048), [0, 4096] splits into 2 boundary edges -> 4 pole triangles.
    assert_eq!(pole_triangles, 4);
}

#[test]
fn closed_bay_crossing_extent_into_buffer_does_not_extrude_false_beam_to_pole() {
    let mut subdivision = options(2);
    subdivision.extend_to_north_pole = true;
    subdivision.extend_to_south_pole = true;
    let mut buffer = VertexBuffers::<ShaderVertex, u32>::new();
    // Closed water bay crossing y = 4096 into the buffer and terminating at a natural tip y = 4145.
    let closed_south_bay = [
        [[1200.0, 3950.0], [1600.0, 3950.0], [1400.0, 4145.0]],
        [[1200.0, 3950.0], [1400.0, 4145.0], [1280.0, 4080.0]],
    ];
    subdivide_appended(&mut buffer, &closed_south_bay, subdivision);

    // Closed island/fjord crossing y = 0 into the northern buffer and terminating at y = -45.
    let closed_north_bay = [[[2200.0, 120.0], [2500.0, 120.0], [2350.0, -45.0]]];
    subdivide_appended(&mut buffer, &closed_north_bay, subdivision);

    assert!(
        pole_vertex_indices(&buffer).is_empty(),
        "closed features dipping into tile buffer must never extrude to poles"
    );
}

#[test]
fn buffered_coastal_bay_clipped_horizontally_at_buffer_rim_does_not_extrude_to_pole() {
    let mut subdivision = options(2);
    subdivision.extend_to_south_pole = true;
    let mut buffer = VertexBuffers::<ShaderVertex, u32>::new();
    // Models the two Ross Sea lobes dipping past 85.0511°S and clipped flat along the MVT
    // south buffer rim (y = 4160) across several hundred units within (0, 4096).
    let ross_sea_lobes = [
        [[600.0, 3900.0], [900.0, 3900.0], [900.0, 4160.0]],
        [[600.0, 3900.0], [900.0, 4160.0], [600.0, 4160.0]],
        [[1050.0, 3900.0], [1350.0, 3900.0], [1350.0, 4160.0]],
        [[1050.0, 3900.0], [1350.0, 4160.0], [1050.0, 4160.0]],
    ];
    subdivide_appended(&mut buffer, &ross_sea_lobes, subdivision);

    assert!(
        pole_vertex_indices(&buffer).is_empty(),
        "sub-tile coastal bays clipped at MVT buffer rim must not extrude wedges to 90°S"
    );
}
