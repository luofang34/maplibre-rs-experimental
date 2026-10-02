//! The autoflight groups' wire contract: what a mode nobody can name
//! must not decode to, which faults take a group down, and which slot
//! each mode byte belongs to.

#![allow(clippy::expect_used, clippy::panic)]

use super::decode_state;
use super::tests::{encode, locate_payload};
use crate::aircraft::AircraftState;

/// Every autoflight enum's unknown value survives a round trip as the
/// fail-closed sentinel rather than laundering into a benign variant.
/// A wire byte nobody can name must not come back as `Off` or `None`,
/// which are things a source can legitimately say.
#[test]
fn unknown_autoflight_bytes_round_trip_as_the_fail_closed_sentinels() {
    use crate::aircraft::Stamped;
    use crate::autopilot::{ApEngagement, ApModes, LateralMode, VerticalMode};
    let state = AircraftState {
        ap_modes: Stamped {
            data: Some(ApModes {
                engagement: ApEngagement::Unknown,
                lateral_active: LateralMode::Unknown,
                lateral_armed: LateralMode::Unknown,
                vertical_active: VerticalMode::Unknown,
                vertical_armed: VerticalMode::Unknown,
            }),
            age_ms: Some(5.0),
        },
        ..AircraftState::default()
    };
    let frame = encode(&state);
    let payload = locate_payload(&frame, 0x14).expect("ap modes present");
    for offset in 0..5 {
        assert_eq!(frame[payload + offset], 255, "unknown byte at {offset}");
    }
    let report = decode_state(&frame).expect("decodes");
    assert_eq!(report.state.ap_modes, state.ap_modes);
}

/// A wire value this build does not know decodes to `Unknown`, never to
/// the nearest variant it does know. `Off` and `None` are claims a
/// source makes; an unreadable byte is not one of them.
#[test]
fn unassigned_autoflight_bytes_decode_to_unknown_not_to_a_benign_variant() {
    use crate::autopilot::{ApEngagement, ApModes, LateralMode, VerticalMode};
    let mut frame = encode(&AircraftState {
        ap_modes: crate::aircraft::Stamped {
            data: Some(ApModes::default()),
            age_ms: Some(5.0),
        },
        ..AircraftState::default()
    });
    let payload = locate_payload(&frame, 0x14).expect("ap modes present");
    // 0x7F is assigned to nothing in any of the three vocabularies.
    for offset in 0..5 {
        frame[payload + offset] = 0x7F;
    }
    let decoded = decode_state(&frame)
        .expect("decodes")
        .state
        .ap_modes
        .data
        .expect("the age is present, so the sample is");
    assert_eq!(decoded.engagement, ApEngagement::Unknown);
    assert_eq!(decoded.lateral_active, LateralMode::Unknown);
    assert_eq!(decoded.lateral_armed, LateralMode::Unknown);
    assert_eq!(decoded.vertical_active, VerticalMode::Unknown);
    assert_eq!(decoded.vertical_armed, VerticalMode::Unknown);
}

/// The five mode bytes sit at five distinct offsets, and swapping any
/// two of them changes the frame. Without this, a codec that read the
/// armed mode where the active one lives would round-trip perfectly
/// and annunciate the wrong tense.
#[test]
fn each_autoflight_mode_byte_has_its_own_offset() {
    use crate::aircraft::Stamped;
    use crate::autopilot::{ApEngagement, ApModes, LateralMode, VerticalMode};
    let state = AircraftState {
        ap_modes: Stamped {
            data: Some(ApModes {
                engagement: ApEngagement::Autopilot,
                lateral_active: LateralMode::Roll,
                lateral_armed: LateralMode::Approach,
                vertical_active: VerticalMode::GlideSlope,
                vertical_armed: VerticalMode::AltitudeCapture,
            }),
            age_ms: Some(5.0),
        },
        ..AircraftState::default()
    };
    let frame = encode(&state);
    let payload = locate_payload(&frame, 0x14).expect("ap modes present");
    let bytes = &frame[payload..payload + 5];
    let mut seen = std::vec::Vec::new();
    for byte in bytes {
        assert!(
            !seen.contains(byte),
            "two mode slots share a byte: {bytes:?}"
        );
        seen.push(*byte);
    }
    assert_eq!(
        decode_state(&frame).expect("decodes").state.ap_modes,
        state.ap_modes
    );
}

/// A target present but not a number faults its group, per field. A
/// display that drew it would show the automation flying toward an
/// infinity.
#[test]
fn non_finite_autoflight_targets_fault_their_group() {
    use crate::autopilot::ApTargets;
    use crate::validate_state;
    let good = ApTargets {
        airspeed_mps: Some(60.0),
        vertical_speed_mps: Some(2.0),
        altitude_m: Some(1000.0),
        ..ApTargets::default()
    };
    let base = |targets: ApTargets| AircraftState {
        ap_targets: targets,
        ..AircraftState::default()
    };
    assert!(validate_state(&base(good)).ap_targets.is_none());
    for bad in [
        ApTargets {
            airspeed_mps: Some(f32::INFINITY),
            ..good
        },
        ApTargets {
            vertical_speed_mps: Some(f32::NEG_INFINITY),
            ..good
        },
        ApTargets {
            altitude_m: Some(f32::INFINITY),
            ..good
        },
    ] {
        assert!(
            validate_state(&base(bad)).ap_targets.is_some(),
            "must fault: {bad:?}"
        );
    }
}

/// Any one unnameable mode faults the group, checked per field so a
/// fault that only ever fired on the engagement byte cannot pass.
#[test]
fn autoflight_mode_faults_fail_the_group_per_branch() {
    use crate::aircraft::Stamped;
    use crate::autopilot::{ApEngagement, ApModes, LateralMode, VerticalMode};
    use crate::validate_state;
    let good = ApModes {
        engagement: ApEngagement::Autopilot,
        lateral_active: LateralMode::Heading,
        lateral_armed: LateralMode::None,
        vertical_active: VerticalMode::Altitude,
        vertical_armed: VerticalMode::None,
    };
    let base = |modes: ApModes| AircraftState {
        ap_modes: Stamped {
            data: Some(modes),
            age_ms: Some(5.0),
        },
        ..AircraftState::default()
    };
    assert!(validate_state(&base(good)).ap_modes.is_none());
    for bad in [
        ApModes {
            engagement: ApEngagement::Unknown,
            ..good
        },
        ApModes {
            lateral_active: LateralMode::Unknown,
            ..good
        },
        ApModes {
            lateral_armed: LateralMode::Unknown,
            ..good
        },
        ApModes {
            vertical_active: VerticalMode::Unknown,
            ..good
        },
        ApModes {
            vertical_armed: VerticalMode::Unknown,
            ..good
        },
    ] {
        assert!(
            validate_state(&base(bad)).ap_modes.is_some(),
            "must fault: {bad:?}"
        );
    }
}

/// The declared target group omits itself when it equals its
/// fail-closed default, and a short payload is refused rather than
/// zero-filled into a target nobody sent.
#[test]
fn autoflight_targets_are_absent_by_default_and_refuse_a_short_payload() {
    let bare = AircraftState::default();
    assert!(
        locate_payload(&encode(&bare), 0x15).is_none(),
        "the default target set is an absent tag"
    );
    let fed = AircraftState {
        ap_targets: crate::autopilot::ApTargets {
            airspeed_mps: Some(60.0),
            ..crate::autopilot::ApTargets::default()
        },
        ..AircraftState::default()
    };
    let frame = encode(&fed);
    let payload = locate_payload(&frame, 0x15).expect("ap targets present");
    let mut truncated = frame[..payload + 19].to_vec();
    let length_at = payload - 2;
    truncated[length_at] = 19;
    truncated[length_at + 1] = 0;
    assert!(
        decode_state(&truncated).is_err(),
        "a payload below the group minimum must be refused"
    );
}
