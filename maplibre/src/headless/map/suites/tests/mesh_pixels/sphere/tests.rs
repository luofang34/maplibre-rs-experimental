use super::*;

#[test]
fn silhouette_extrema_include_edge_crossings_missed_by_five_samples() {
    // A small disc intersects the top edge away from its midpoint and both corners.
    let q = [-1.0, 0.0, -1.0, 0.4, 2.1, -1.1325];
    for (x, y) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0), (0.5, 0.5)] {
        let [a, b, c, d, e, f] = q;
        assert!(a * x * x + b * x * y + c * y * y + d * x + e * y + f < 0.0);
    }
    let (_, maximum) = extrema(q, [0.0, 1.0, 0.0, 1.0]);
    assert!((maximum - 0.0075).abs() < 1e-12);
}
