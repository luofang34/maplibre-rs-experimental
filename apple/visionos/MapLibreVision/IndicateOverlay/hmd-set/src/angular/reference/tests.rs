use super::*;
use indicate_instrument_state::{AircraftState, FreshnessPolicy, Sig, resolve};

#[test]
fn unavailable_attitude_yields_a_level_track_frame_without_claiming_aircraft_alignment() {
    let mut data = resolve(&AircraftState::default(), &FreshnessPolicy::default());
    assert_eq!(view_reference(&data).kind, 0);
    data.track_rad = Sig::valid(core::f32::consts::FRAC_PI_2);
    let frame = view_reference(&data);
    assert_eq!(frame.kind, 1);
    assert!(dot(frame.forward, [1.0, 0.0, 0.0]) > 0.999);
    assert!(dot(frame.up, [0.0, 0.0, 1.0]) > 0.999);
}

#[test]
fn measured_frame_tracks_bank_and_rejects_nonfinite_angles() {
    let mut data = resolve(&AircraftState::default(), &FreshnessPolicy::default());
    data.heading.reference = HeadingReference::True;
    data.heading.value_rad = Sig::valid(0.0);
    data.pitch_rad = Sig::valid(20_f32.to_radians());
    data.roll_rad = Sig::valid(45_f32.to_radians());
    let frame = view_reference(&data);
    assert_eq!(frame.kind, 2);
    assert!(frame.right[2] < -0.6, "positive bank lowers the right wing");
    assert!(frame.forward[2] > 0.3, "positive pitch raises the fuselage");
    for axis in [frame.right, frame.up, frame.forward] {
        assert!((dot(axis, axis) - 1.0).abs() < 1e-6);
    }
    assert!(dot(frame.right, frame.forward).abs() < 1e-6);
    data.pitch_rad = Sig::valid(f32::NAN);
    assert_eq!(view_reference(&data).kind, 0);
}
