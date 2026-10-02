//! Shared guidance safety and compatibility behavior.
#![allow(clippy::expect_used, clippy::panic)]

use super::{envelope::*, guidance::*, presentation::*, speed::*};
use crate::{FreshnessPolicy, SignalStatus, abi::v8::fixtures, resolve};

#[test]
fn only_valid_commands_and_attitude_produce_steering() {
    let state = fixtures::extended();
    let data = resolve(&state, &FreshnessPolicy::default());
    assert!(attitude_error(&data).is_some());
    for status in [
        SignalStatus::Stale,
        SignalStatus::Degraded,
        SignalStatus::Missing,
        SignalStatus::Failed,
    ] {
        let mut command_bad = data;
        command_bad.flight.guidance.status = status;
        assert!(attitude_error(&command_bad).is_none());
        let mut attitude_bad = data;
        attitude_bad.roll_rad.status = status;
        assert!(attitude_error(&attitude_bad).is_none());
        let mut legacy_bad = data;
        legacy_bad.flight.guidance_present = false;
        legacy_bad.flight.director.status = status;
        assert!(attitude_error(&legacy_bad).is_none());
    }
    let mut unusual = data;
    unusual.presentation.unusual = true;
    assert!(attitude_error(&unusual).is_none());
}

#[test]
fn invalid_extended_report_never_falls_back_to_legacy() {
    let mut state = fixtures::extended();
    state
        .flight
        .guidance
        .data
        .as_mut()
        .expect("guidance")
        .director = crate::FdEngagement::Unknown;
    let data = resolve(&state, &FreshnessPolicy::default());
    assert_eq!(data.flight.director.status, SignalStatus::Valid);
    assert_ne!(guidance(&data).status, SignalStatus::Valid);
    assert!(attitude_error(&data).is_none());
    assert!(effective_speed(&data).is_none());
}

#[test]
fn effective_projection_requires_the_same_computation_and_native_target() {
    let mut data = resolve(&fixtures::extended(), &FreshnessPolicy::default());
    assert!(effective_speed_projection(&data).is_some());
    data.flight.speed.value.guidance_identity.sequence = 41;
    assert!(effective_speed_projection(&data).is_none());
    data.flight.speed.value.guidance_identity = data.flight.guidance.value.identity;
    data.flight
        .speed
        .value
        .effective
        .as_mut()
        .expect("target")
        .native
        .value = 0.79;
    assert!(effective_speed_projection(&data).is_none());
    assert_eq!(effective_speed(&data).expect("native target").value, 0.78);
    assert_eq!(
        selected_speed(&data)
            .expect("selected target")
            .native
            .coordinate,
        SpeedCoordinate::Ias
    );
}

#[test]
fn malformed_envelope_does_not_remove_measured_speed() {
    let mut state = fixtures::extended();
    state
        .flight
        .speed
        .data
        .as_mut()
        .expect("speed")
        .envelope
        .intervals[0]
        .as_mut()
        .expect("interval")
        .lower = f32::NAN;
    let data = resolve(&state, &FreshnessPolicy::default());
    assert_eq!(current_speed(&data).1.status, SignalStatus::Valid);
    assert_eq!(data.flight.envelope_status, SignalStatus::Failed);
}

#[test]
fn unit_conversion_overflow_invalidates_only_the_envelope() {
    let mut state = fixtures::extended();
    state
        .flight
        .speed
        .data
        .as_mut()
        .expect("speed")
        .envelope
        .intervals[0]
        .as_mut()
        .expect("interval")
        .upper = f32::MAX;
    let data = resolve(&state, &FreshnessPolicy::default());
    assert_eq!(data.flight.speed.status, SignalStatus::Valid);
    assert_eq!(data.flight.envelope_status, SignalStatus::Failed);
}

#[test]
fn partial_empty_and_disconnected_envelopes_remain_distinct() {
    let mut state = fixtures::extended();
    let speed = state.flight.speed.data.as_mut().expect("speed");
    speed.envelope.intervals[1] = Some(EnvelopeInterval {
        kind: EnvelopeKind::Operating,
        lower: 175.0,
        upper: 181.0,
        lower_reason: BoundaryReason::Control,
        upper_reason: BoundaryReason::Thermal,
    });
    assert!(super::envelope_fault(&speed.envelope).is_none());
    speed.envelope.intervals[1]
        .as_mut()
        .expect("interval")
        .lower = 150.0;
    assert!(super::envelope_fault(&speed.envelope).is_some());
    speed.envelope.intervals = [None; MAX_ENVELOPE_INTERVALS];
    assert!(super::envelope_fault(&speed.envelope).is_none());
    speed.envelope.coverage = EnvelopeCoverage::Partial;
    assert!(super::envelope_fault(&speed.envelope).is_none());
}

#[test]
fn transition_attention_uses_producer_elapsed_time_plus_age() {
    let mut data = resolve(&fixtures::extended(), &FreshnessPolicy::default());
    assert!(transition_visible(&data, 1000.0));
    data.flight.guidance_age_ms = Some(250.0);
    assert!(!transition_visible(&data, 1000.0));
    data.flight.guidance.value.transition.reason = TransitionReason::None;
    data.flight.guidance_age_ms = Some(0.0);
    assert!(!transition_visible(&data, 1000.0));
}

#[test]
fn ap_fd_and_thrust_engagement_remain_independent() {
    let mut state = fixtures::extended();
    let sample = state.flight.guidance.data.as_mut().expect("guidance");
    sample.director = crate::FdEngagement::Off;
    let data = resolve(&state, &FreshnessPolicy::default());
    assert_eq!(data.flight.guidance.status, SignalStatus::Valid);
    assert_eq!(
        guidance(&data).sample.autopilot,
        crate::FdEngagement::Engaged
    );
    assert!(attitude_error(&data).is_none());
    assert!(effective_speed(&data).is_some());
    let sample = state.flight.guidance.data.as_mut().expect("guidance");
    sample.autopilot = crate::FdEngagement::Off;
    sample.director = crate::FdEngagement::Engaged;
    assert!(attitude_error(&resolve(&state, &FreshnessPolicy::default())).is_some());
    let sample = state.flight.guidance.data.as_mut().expect("guidance");
    sample.director = crate::FdEngagement::Off;
    assert!(
        super::guidance_fault(sample).is_some(),
        "active attitude modes require a controlling channel"
    );
}

#[test]
fn legacy_modes_can_report_fd_engagement_without_inventing_commands() {
    let mut state = fixtures::extended();
    state.flight = Default::default();
    state.director = Default::default();
    state.ap_modes.data.as_mut().expect("modes").engagement = crate::ApEngagement::FlightDirector;
    let data = resolve(&state, &FreshnessPolicy::default());
    assert_eq!(
        legacy_director_state(&data),
        (
            crate::FdEngagement::Engaged,
            SignalStatus::Valid,
            crate::GroupId::ApModes
        )
    );
    assert!(attitude_error(&data).is_none());
}
