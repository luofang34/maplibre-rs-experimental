use super::*;

/// The vertex of the mesh nearest `[s, t]` of the picture.
fn place(triangles: &[[Vertex; 3]], [s, t]: [f64; 2]) -> [f64; 2] {
    triangles
        .iter()
        .flatten()
        .find(|(_, texture)| {
            (texture[0] / texture[2] - s).abs() < 1e-9 && (texture[1] / texture[2] - t).abs() < 1e-9
        })
        .map(|(position, _)| *position)
        .expect("a vertex there")
}

const SQUARE: [[f64; 2]; 4] = [[0.25, 0.25], [0.5, 0.25], [0.5, 0.5], [0.25, 0.5]];

#[test]
fn a_rectangle_is_two_triangles_with_its_corners_at_the_pictures() {
    let triangles = triangles(SQUARE);
    assert_eq!(triangles.len(), 2);
    for (corner, st) in SQUARE
        .iter()
        .zip([[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]])
    {
        assert_eq!(&place(&triangles, st), corner);
    }
    assert!(triangles
        .iter()
        .flatten()
        .all(|(_, texture)| texture[2] == 1.0));
}

#[test]
fn a_trapezoid_is_the_perspective_view_of_the_picture() {
    // A trapezoid narrower at the top, as a photograph of the ground looking north is.
    let trapezoid = [[0.3, 0.25], [0.45, 0.25], [0.5, 0.5], [0.25, 0.5]];
    let triangles = triangles(trapezoid);
    assert_eq!(triangles.len(), 2, "a projective warp needs no subdivision");
    // The picture's centre lands where the trapezoid's diagonals cross, not halfway down it.
    let mid = vertex(
        rounded_corners(trapezoid),
        terms(rounded_corners(trapezoid)),
        [0.5, 0.5],
    )
    .0;
    // Diagonals: top left to bottom right and top right to bottom left.
    let cross = |[a, b]: [[f64; 2]; 2], [c, d]: [[f64; 2]; 2]| {
        let r = [b[0] - a[0], b[1] - a[1]];
        let q = [d[0] - c[0], d[1] - c[1]];
        let t = ((c[0] - a[0]) * q[1] - (c[1] - a[1]) * q[0]) / (r[0] * q[1] - r[1] * q[0]);
        [a[0] + t * r[0], a[1] + t * r[1]]
    };
    let [top_left, top_right, bottom_right, bottom_left] = rounded_corners(trapezoid);
    let crossing = cross([top_left, bottom_right], [top_right, bottom_left]);
    assert!(
        (mid[0] - crossing[0]).abs() < 1e-9 && (mid[1] - crossing[1]).abs() < 1e-9,
        "{mid:?} {crossing:?}"
    );
    assert!(
        mid[1] < 0.375,
        "the far, narrow half of the picture is drawn smaller, so its middle is nearer the top"
    );
}

#[test]
fn a_quad_near_a_triangle_blends_towards_a_rubber_sheet_on_a_grid() {
    let near_triangle = [[0.25, 0.25], [0.5, 0.25], [0.5, 0.5], [0.2501, 0.2501]];
    let corners = rounded_corners(near_triangle);
    let blend = terms(corners).blend;
    assert!(blend > 0.0, "{blend}");
    assert_eq!(
        triangles(near_triangle).len(),
        2 * SUBDIVIDED_QUAD_GRANULARITY * SUBDIVIDED_QUAD_GRANULARITY
    );
}

#[test]
fn corners_rounded_onto_a_rectangle_count_as_one() {
    // Off by far less than a unit of the tile the picture is centred in.
    let nearly = [[0.25, 0.25], [0.5 + 1e-12, 0.25], [0.5, 0.5], [0.25, 0.5]];
    assert_eq!(terms(rounded_corners(nearly)), BILINEAR);
    assert_eq!(triangles(nearly).len(), 2);
}
