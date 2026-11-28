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

#[test]
fn a_frame_s_measurement_is_shared_among_the_drapes_it_drew() {
    let mut cost = DrapeCost::default();
    cost.measured_frame(Duration::from_millis(12), 4);
    assert_eq!(cost.per_drape(), Duration::from_millis(3));
    assert_eq!(cost.last_frame(), Some((Duration::from_millis(12), 4)));
}

#[test]
fn cpu_fallback_enforces_assumed_floor_and_scales_heavy_encode_passes() {
    let slot = DrapeTimerSlot::default();
    assert_eq!(slot.take_cpu_fallback(), None);

    slot.record_encode_cpu(Duration::from_micros(200));
    slot.record_pass_cpu(Duration::from_micros(300), 3);
    assert_eq!(
        slot.take_cpu_fallback(),
        Some((ASSUMED_DRAPE_COST * 3, 3)),
        "fast CPU encode still enforces ASSUMED_DRAPE_COST floor per drape"
    );
    assert_eq!(slot.take_cpu_fallback(), None, "consumed once per frame");

    slot.record_encode_cpu(Duration::from_millis(3));
    slot.record_pass_cpu(Duration::from_millis(4), 2);
    assert_eq!(
        slot.take_cpu_fallback(),
        Some((Duration::from_millis(14), 2)),
        "heavy CPU pass encoding scales 2x above floor"
    );
}
