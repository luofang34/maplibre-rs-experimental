use super::*;

#[test]
fn grid_matches_exhaustive_collision_for_dense_and_offscreen_labels() {
    let mut grid = CollisionGrid::new(1920.0, 1080.0);
    let mut rectangles = Vec::new();
    for i in 0..2000 {
        let x = f64::from((i * 83) % 2400) - 240.0;
        let y = f64::from((i * 47) % 1400) - 160.0;
        let rect = [x, y, x + 80.0, y + 24.0];
        assert_eq!(
            grid.overlaps(rect),
            rectangles.iter().any(|&other| intersects(rect, other))
        );
        if !grid.overlaps(rect) {
            grid.insert(rect);
            rectangles.push(rect);
        }
    }
    let enormous = [-1e20, -1e20, 1e20, 1e20];
    assert!(grid.overlaps(enormous));
    grid.insert(enormous);
    assert!(
        grid.cells.len() <= 32 * 19,
        "cell count is bounded by the viewport"
    );
}
