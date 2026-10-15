use super::*;

#[test]
fn replacing_obstacles_advances_the_revision_only_when_they_change() {
    let mut obstacles = LabelObstacles::default();
    assert!(obstacles.replace(vec![[10.0, 10.0, 20.0, 20.0]]));
    assert_eq!(obstacles.revision(), 1);
    assert!(!obstacles.replace(vec![[10.0, 10.0, 20.0, 20.0]]));
    assert_eq!(obstacles.revision(), 1);
    assert!(obstacles.replace(vec![[10.0, 10.0, f64::NAN, 20.0], [5.0, 5.0, 1.0, 1.0]]));
    assert!(
        obstacles.boxes().is_empty(),
        "invalid rectangles are dropped"
    );
}
