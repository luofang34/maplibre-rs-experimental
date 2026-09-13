use super::*;
use indicate_instrument_state::{AircraftState, FreshnessPolicy, Sig, resolve};

#[test]
fn head_cones_hysteresis_and_unusual_attitude_share_one_policy() {
    let mut data = resolve(&AircraftState::default(), &FreshnessPolicy::default());
    assert!(use_compact(&data, 1.0, false));
    data.track_rad = Sig::valid(0.0);
    assert!(!use_compact(&data, 1.0, false));
    let between = libm::cosf(31_f32.to_radians());
    assert!(!use_compact(&data, between, false));
    assert!(use_compact(&data, between, true));
    assert!(!use_compact(&data, libm::cosf(27_f32.to_radians()), true));
    assert!(use_compact(&data, libm::cosf(36_f32.to_radians()), false));
    assert!(use_compact(&data, f32::NAN, false));
    data.presentation.unusual = true;
    assert!(use_compact(&data, 1.0, false));
}
