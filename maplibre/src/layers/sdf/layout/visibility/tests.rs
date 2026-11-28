use super::distance_opacity;
#[test]
fn relevance_fades_smoothly_before_the_distance_limit() {
    assert_eq!(distance_opacity(50_000.0, 80_000.0), 1.0);
    assert_eq!(distance_opacity(70_000.0, 80_000.0), 0.5);
    assert_eq!(distance_opacity(80_000.0, 80_000.0), 0.0);
    assert_eq!(distance_opacity(800_000.0, 80_000.0), 0.0);
    assert!(
        (distance_opacity(70_001.0, 80_000.0) - distance_opacity(70_000.0, 80_000.0)).abs() < 0.001
    );
}
