use crate::projection::globe::subdivision::subdivide_line_segment_on_grid;

/// Width of a terrain mesh cell in tile units.
const CELL: f32 = 32.0;

#[test]
fn a_segment_inside_one_cell_crosses_its_diagonal_once() {
    let points = subdivide_line_segment_on_grid([4.0, 20.0], [28.0, 12.0], 128, true)
        .expect("subdivide inside a cell");
    // x - y runs from -16 to 16 and is zero on the diagonal, at x = 16 and y = 16.
    assert_eq!(points, vec![[16.0, 16.0], [28.0, 12.0]]);
}

#[test]
fn every_piece_stays_on_one_face_of_the_mesh() {
    let points = subdivide_line_segment_on_grid([3.0, 1000.0], [700.0, 90.0], 128, true)
        .expect("subdivide across cells");
    let mut start = [3.0_f32, 1000.0];
    for end in points {
        let middle = [(start[0] + end[0]) / 2.0, (start[1] + end[1]) / 2.0];
        let cell = [(middle[0] / CELL).floor(), (middle[1] / CELL).floor()];
        let lower = |point: [f32; 2]| point[1] - cell[1] * CELL >= point[0] - cell[0] * CELL;
        for point in [start, end] {
            for axis in 0..2 {
                let local = point[axis] / CELL - cell[axis];
                assert!(
                    (-1e-3..=1.0 + 1e-3).contains(&local),
                    "{point:?} leaves cell {cell:?}"
                );
            }
            let off_diagonal = (point[1] - cell[1] * CELL) - (point[0] - cell[0] * CELL);
            assert!(
                off_diagonal.abs() < 1e-2 || lower(point) == lower(middle),
                "piece {start:?}..{end:?} crosses the diagonal of cell {cell:?}"
            );
        }
        start = end;
    }
}
