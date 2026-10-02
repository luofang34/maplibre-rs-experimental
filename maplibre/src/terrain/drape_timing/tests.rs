use super::*;

#[test]
fn a_moving_frame_draws_the_drapes_its_time_holds() {
    let mut cost = DrapeCost::default();
    assert_eq!(cost.per_drape(), ASSUMED_DRAPE_COST);
    assert_eq!(cost.drapes_within(Duration::from_millis(4), 24), 2);
    cost.measured(Duration::from_millis(1));
    assert_eq!(cost.drapes_within(Duration::from_millis(4), 24), 4);
    assert_eq!(
        cost.drapes_within(Duration::from_millis(4), 3),
        3,
        "never past the limit"
    );
    cost.measured(Duration::from_millis(9));
    assert_eq!(
        cost.per_drape(),
        Duration::from_millis(3),
        "a running average"
    );
    assert_eq!(
        cost.drapes_within(Duration::from_millis(1), 24),
        1,
        "a moving view still refines one drape a frame"
    );
}
