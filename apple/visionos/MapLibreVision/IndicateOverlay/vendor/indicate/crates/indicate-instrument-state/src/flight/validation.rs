//! Validation keeps bad envelope geometry separate from measured speed.

use super::{envelope::*, guidance::*, speed::*};
use crate::{FdEngagement, GroupFault, LateralMode, VerticalMode};

pub(crate) fn guidance_fault(sample: &GuidanceSample) -> Option<GroupFault> {
    let unknown = [sample.autopilot, sample.director, sample.autothrust]
        .contains(&FdEngagement::Unknown)
        || [sample.lateral.active, sample.lateral.armed].contains(&LateralMode::Unknown)
        || [sample.vertical.active, sample.vertical.armed].contains(&VerticalMode::Unknown)
        || [sample.thrust.active, sample.thrust.armed].contains(&ThrustMode::Unknown)
        || sample.speed_control == SpeedControl::Unknown
        || sample.protection == ProtectionState::Unknown
        || sample.transition.reason == TransitionReason::Unknown;
    if unknown {
        return Some(GroupFault::UnknownEnum);
    }
    if !sample.identity.is_declared() {
        return Some(GroupFault::SourceAbsent);
    }
    if sample.speed_target.is_some_and(|v| !v.is_valid())
        || sample.command.is_some_and(|v| !command_valid(v))
        || sample
            .transition
            .elapsed_ms
            .is_some_and(|v| !v.is_finite() || v < 0.0)
    {
        return Some(GroupFault::NonFinite);
    }
    let bad_control = match sample.speed_control {
        SpeedControl::Pitch => {
            sample.vertical.active != VerticalMode::Airspeed
                || (sample.autopilot != FdEngagement::Engaged
                    && sample.director != FdEngagement::Engaged)
        }
        SpeedControl::Thrust => {
            sample.autothrust != FdEngagement::Engaged || sample.thrust.active != ThrustMode::Speed
        }
        _ => false,
    };
    if bad_control
        || inactive_mode_claim(sample)
        || (sample.speed_control != SpeedControl::None && sample.speed_target.is_none())
    {
        return Some(GroupFault::SourceAbsent);
    }
    if sample.transition.reason != TransitionReason::None && sample.transition.elapsed_ms.is_none()
    {
        return Some(GroupFault::SourceAbsent);
    }
    None
}

fn inactive_mode_claim(sample: &GuidanceSample) -> bool {
    let attitude_control =
        sample.autopilot == FdEngagement::Engaged || sample.director == FdEngagement::Engaged;
    (!attitude_control
        && (sample.lateral.active != LateralMode::None
            || sample.vertical.active != VerticalMode::None))
        || (sample.autothrust != FdEngagement::Engaged && sample.thrust.active != ThrustMode::None)
}

pub(super) fn command_valid(command: AttitudeCommand) -> bool {
    command.pitch_rad.is_finite()
        && command.roll_rad.is_finite()
        && command.pitch_rad.abs() <= core::f32::consts::FRAC_PI_2
        && command.roll_rad.abs() <= core::f32::consts::PI
}

pub(super) fn target_valid(target: ProjectedTarget) -> bool {
    target.native.is_valid() && target.projected.is_none_or(|v| v.is_finite() && v >= 0.0)
}

pub(crate) fn speed_fault(sample: &SpeedSample) -> Option<GroupFault> {
    if sample.coordinate == SpeedCoordinate::Unknown {
        return Some(GroupFault::UnknownEnum);
    }
    if sample.current.is_some_and(|v| !v.is_finite() || v < 0.0)
        || sample
            .current
            .is_some_and(|v| !sample.coordinate.display_value(v).is_finite())
    {
        return Some(GroupFault::NonFinite);
    }
    None
}

pub(crate) fn envelope_fault(envelope: &ProjectedEnvelope) -> Option<GroupFault> {
    if envelope.coverage == EnvelopeCoverage::Unknown {
        return Some(GroupFault::UnknownEnum);
    }
    if envelope.coverage == EnvelopeCoverage::Unavailable {
        return envelope
            .intervals
            .iter()
            .any(Option::is_some)
            .then_some(GroupFault::SourceAbsent);
    }
    if envelope.model_id == 0 || envelope.condition_id == 0 {
        return Some(GroupFault::SourceAbsent);
    }
    let mut upper = [None; 5];
    for interval in envelope.intervals.iter().flatten() {
        if interval.kind == EnvelopeKind::Unknown
            || interval.lower_reason == BoundaryReason::Unknown
            || interval.upper_reason == BoundaryReason::Unknown
        {
            return Some(GroupFault::UnknownEnum);
        }
        // A kind this table has no slot for is treated as unknown rather
        // than indexed: a variant added past the table must fail closed,
        // never trap.
        let Some(slot) = upper.get_mut(interval.kind as usize) else {
            return Some(GroupFault::UnknownEnum);
        };
        if !interval.lower.is_finite()
            || !interval.upper.is_finite()
            || interval.lower < 0.0
            || interval.lower > interval.upper
            || slot.is_some_and(|v| interval.lower <= v)
        {
            return Some(GroupFault::NonFinite);
        }
        *slot = Some(interval.upper);
    }
    None
}

pub(crate) fn speed_envelope_fault(sample: &SpeedSample) -> Option<GroupFault> {
    envelope_fault(&sample.envelope).or_else(|| {
        sample
            .envelope
            .intervals
            .iter()
            .flatten()
            .any(|bounds| {
                !sample.coordinate.display_value(bounds.lower).is_finite()
                    || !sample.coordinate.display_value(bounds.upper).is_finite()
            })
            .then_some(GroupFault::NonFinite)
    })
}
