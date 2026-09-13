#![allow(clippy::expect_used)]
use super::*;
#[test]
fn nose_up_moves_horizon_down_and_right_bank_raises_right_end() {
    let (a, b) = horizon(Sig::valid(0.0), Sig::valid(0.2)).expect("measured pitch");
    assert!(a[1] > 0.0 && b[1] > 0.0);
    let (a, b) = horizon(Sig::valid(0.3), Sig::valid(0.0)).expect("measured roll");
    assert!(a[1] > 0.0 && b[1] < 0.0);
    for pitch in [
        -core::f32::consts::FRAC_PI_2,
        0.0,
        core::f32::consts::FRAC_PI_2,
    ] {
        let (a, b) = horizon(Sig::valid(2.0), Sig::valid(pitch)).expect("bounded attitude");
        for p in [a, b] {
            assert!((p[0] * p[0] + p[1] * p[1] - RADIUS * RADIUS).abs() < 0.01);
        }
    }
}
#[test]
fn track_only_recordings_never_receive_a_level_attitude_symbol() {
    assert!(horizon(Sig::missing(), Sig::missing()).is_none());
    assert!(horizon(Sig::valid(0.0), Sig::missing()).is_none());
    assert!(horizon(Sig::valid(f32::NAN), Sig::valid(0.0)).is_none());
}
