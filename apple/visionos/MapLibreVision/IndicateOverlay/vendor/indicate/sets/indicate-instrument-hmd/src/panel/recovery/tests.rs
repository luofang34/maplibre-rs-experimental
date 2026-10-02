#![allow(clippy::expect_used)]
use super::*;
use indicate_instrument_state::{
    AircraftState, AirframeDisplayProfile, FreshnessPolicy, Quat, UnusualAttitudeState, resolve,
};

fn data(roll: f32, pitch: f32, state: &mut UnusualAttitudeState) -> PanelData {
    let mut data = resolve(&AircraftState::default(), &FreshnessPolicy::default());
    data.roll_rad = Sig::valid(roll.to_radians());
    data.pitch_rad = Sig::valid(pitch.to_radians());
    data.presentation = state.step(
        Quat::from_euler(roll.to_radians(), pitch.to_radians(), 0.0),
        &AirframeDisplayProfile::simulator(),
    );
    data
}

#[test]
fn chevrons_point_toward_the_horizon_through_bank_and_inversion() {
    for roll in [-180.0, -120.0, -60.0, 0.0, 60.0, 120.0, 180.0] {
        for pitch in [-89.0, -40.0, 55.0, 89.0] {
            let data = data(roll, pitch, &mut UnusualAttitudeState::default());
            let toward = pitch.signum();
            let down = [data.roll_rad.value.sin(), data.roll_rad.value.cos()];
            for points in geometry(&data).expect("extreme pitch has orientation cues") {
                let base = [
                    (points[0][0] + points[2][0]) / 2.0,
                    (points[0][1] + points[2][1]) / 2.0,
                ];
                let direction = [points[1][0] - base[0], points[1][1] - base[1]];
                assert!(toward * (direction[0] * down[0] + direction[1] * down[1]) > 14.9);
                for point in points {
                    assert!((point[0] - 600.0).hypot(point[1] - 405.0) < 68.0);
                }
            }
        }
    }
}

#[test]
fn recovery_uses_latched_profile_thresholds_and_requires_live_attitude() {
    let mut state = UnusualAttitudeState::default();
    assert!(geometry(&data(0.0, 49.0, &mut state)).is_none());
    assert!(geometry(&data(0.0, 51.0, &mut state)).is_some());
    assert!(geometry(&data(0.0, 47.0, &mut state)).is_some());
    assert!(geometry(&data(0.0, 44.0, &mut state)).is_none());
    for status in [
        SignalStatus::Missing,
        SignalStatus::Stale,
        SignalStatus::Degraded,
        SignalStatus::Failed,
    ] {
        let mut data = data(40.0, 60.0, &mut state);
        data.roll_rad.status = status;
        assert!(geometry(&data).is_none());
    }
}
