//! Deterministic extended guidance and envelope inputs for integration review.

use crate::flight::{FlightInputs, SolutionId, envelope::*, guidance::*, speed::*};
use crate::{AircraftState, FdEngagement, LateralMode, VerticalMode};

/// Level cruise with a Mach target projected onto an IAS coordinate.
pub fn extended() -> AircraftState {
    let mut state = super::full();
    state.attitude = super::attitude([1.0, 0.0, 0.0, 0.0], [0.0; 3], 40.0);
    let identity = SolutionId {
        source: 17,
        epoch: 3,
        sequence: 42,
    };
    let target = SpeedValue {
        coordinate: SpeedCoordinate::Mach,
        value: 0.78,
    };
    state.flight = FlightInputs {
        guidance: super::stamped(
            GuidanceSample {
                identity,
                autopilot: FdEngagement::Engaged,
                director: FdEngagement::Engaged,
                autothrust: FdEngagement::Engaged,
                lateral: ModePair {
                    active: LateralMode::Nav,
                    armed: LateralMode::Approach,
                },
                vertical: ModePair {
                    active: VerticalMode::Altitude,
                    armed: VerticalMode::GlideSlope,
                },
                thrust: ModePair {
                    active: ThrustMode::Speed,
                    armed: ThrustMode::None,
                },
                speed_control: SpeedControl::Thrust,
                speed_target: Some(target),
                command: Some(AttitudeCommand {
                    pitch_rad: 0.05,
                    roll_rad: -0.1,
                }),
                protection: ProtectionState::Inactive,
                transition: GuidanceTransition {
                    reason: TransitionReason::Capture,
                    elapsed_ms: Some(750.0),
                },
            },
            40.0,
        ),
        speed: super::stamped(
            SpeedSample {
                guidance_identity: identity,
                coordinate: SpeedCoordinate::Ias,
                current: Some(145.0),
                rate: Some(0.4),
                secondary: Some(SpeedValue {
                    coordinate: SpeedCoordinate::Mach,
                    value: 0.77,
                }),
                selected: Some(ProjectedTarget {
                    native: SpeedValue {
                        coordinate: SpeedCoordinate::Ias,
                        value: 155.0,
                    },
                    projected: Some(155.0),
                }),
                effective: Some(ProjectedTarget {
                    native: target,
                    projected: Some(147.0),
                }),
                envelope: envelope(),
            },
            45.0,
        ),
    };
    state
}

fn envelope() -> ProjectedEnvelope {
    let mut intervals = [None; MAX_ENVELOPE_INTERVALS];
    intervals[0] = Some(EnvelopeInterval {
        kind: EnvelopeKind::Operating,
        lower: 126.0,
        upper: 161.0,
        lower_reason: BoundaryReason::Maneuver,
        upper_reason: BoundaryReason::Mmo,
    });
    intervals[1] = Some(EnvelopeInterval {
        kind: EnvelopeKind::Preferred,
        lower: 140.0,
        upper: 150.0,
        lower_reason: BoundaryReason::Mission,
        upper_reason: BoundaryReason::Mission,
    });
    ProjectedEnvelope {
        coverage: EnvelopeCoverage::Complete,
        model_id: 19,
        condition_id: 81,
        intervals,
    }
}
