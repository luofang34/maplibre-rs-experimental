use super::*;
#[test]
fn rounded_north_wraps_and_missing_stays_missing() {
    assert_eq!(degrees(Sig::valid(359.8_f32.to_radians())).value, 0.0);
    assert_eq!(degrees(Sig::valid(-1_f32.to_radians())).value, 359.0);
    assert_eq!(degrees(Sig::missing()).status, SignalStatus::Missing);
}
