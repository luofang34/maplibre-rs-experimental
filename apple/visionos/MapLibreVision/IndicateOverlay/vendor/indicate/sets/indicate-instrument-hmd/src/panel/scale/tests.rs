use super::*;
#[test]
fn trend_requires_live_airspeed_and_supplied_rate() {
    assert_eq!(speed_trend(Sig::valid(100.0), Sig::valid(2.0)), Some(12.0));
    assert_eq!(
        speed_trend(Sig::valid(100.0), Sig::valid(-20.0)),
        Some(-30.0)
    );
    for status in [
        SignalStatus::Missing,
        SignalStatus::Stale,
        SignalStatus::Degraded,
        SignalStatus::Failed,
    ] {
        assert_eq!(
            speed_trend(Sig::with_status(100.0, status), Sig::valid(2.0)),
            None
        );
        assert_eq!(
            speed_trend(Sig::valid(100.0), Sig::with_status(2.0, status)),
            None
        );
    }
    assert_eq!(speed_trend(Sig::valid(f32::NAN), Sig::valid(2.0)), None);
}
