//! Pure shared decisions for modes, steering cues, and speed targets.

use super::{
    guidance::*,
    speed::*,
    validation::{command_valid, target_valid},
};
use crate::{ApEngagement, FdEngagement, GroupId, PanelData, Sig, SignalStatus};

/// Signed aircraft attitude errors for fly-to command bars.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AttitudeError {
    /// Pitch error in radians, positive nose-up.
    pub pitch_rad: f32,
    /// Shortest bank error in radians, positive right.
    pub roll_rad: f32,
}

/// One canonical mode report with its source attribution.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GuidancePresentation {
    /// Canonical mode and engagement values.
    pub sample: GuidanceSample,
    /// Status of the mode report.
    pub status: SignalStatus,
    /// Source group for attributed mode text.
    pub group: GroupId,
    /// Whether the source supplies the extended contract.
    pub extended: bool,
}

/// Resolves extended authority or adapts the legacy mode report.
pub fn guidance(data: &PanelData) -> GuidancePresentation {
    let flight = &data.flight;
    if flight.guidance_present {
        return GuidancePresentation {
            sample: flight.guidance.value,
            status: flight.guidance.status,
            group: GroupId::Guidance,
            extended: true,
        };
    }
    let modes = flight.ap_modes.value;
    GuidancePresentation {
        sample: GuidanceSample {
            autopilot: if modes.engagement == ApEngagement::Autopilot {
                FdEngagement::Engaged
            } else {
                FdEngagement::Off
            },
            director: legacy_director_state(data).0,
            autothrust: FdEngagement::Off,
            lateral: ModePair {
                active: modes.lateral_active,
                armed: modes.lateral_armed,
            },
            vertical: ModePair {
                active: modes.vertical_active,
                armed: modes.vertical_armed,
            },
            ..GuidanceSample::default()
        },
        status: flight.ap_modes.status,
        group: GroupId::ApModes,
        extended: false,
    }
}

/// Resolves legacy FD engagement without assuming that AP engagement also enables the FD.
pub fn legacy_director_state(data: &PanelData) -> (FdEngagement, SignalStatus, GroupId) {
    let flight = &data.flight;
    if flight.director.status != SignalStatus::Missing {
        return (
            flight.director.engagement,
            flight.director.status,
            GroupId::FlightDirector,
        );
    }
    let value = match flight.ap_modes.value.engagement {
        ApEngagement::FlightDirector => FdEngagement::Engaged,
        ApEngagement::Off => FdEngagement::Off,
        _ => {
            return (
                FdEngagement::Unknown,
                SignalStatus::Missing,
                GroupId::ApModes,
            );
        }
    };
    (value, flight.ap_modes.status, GroupId::ApModes)
}

/// Computes command errors only when both command and measured attitude are valid.
pub fn attitude_error(data: &PanelData) -> Option<AttitudeError> {
    if data.presentation.unusual
        || data.roll_rad.status != SignalStatus::Valid
        || data.pitch_rad.status != SignalStatus::Valid
        || !data.roll_rad.value.is_finite()
        || !data.pitch_rad.value.is_finite()
    {
        return None;
    }
    let flight = &data.flight;
    let command = if flight.guidance_present {
        let sample = flight.guidance.value;
        if flight.guidance.status != SignalStatus::Valid || sample.director != FdEngagement::Engaged
        {
            return None;
        }
        sample.command?
    } else {
        let fd = flight.director;
        if fd.status != SignalStatus::Valid || fd.engagement != FdEngagement::Engaged {
            return None;
        }
        AttitudeCommand {
            pitch_rad: fd.pitch_cmd_rad,
            roll_rad: fd.roll_cmd_rad,
        }
    };
    if !command_valid(command) {
        return None;
    }
    Some(AttitudeError {
        pitch_rad: command.pitch_rad - data.pitch_rad.value,
        roll_rad: crate::shortest_angle_rad(data.roll_rad.value, command.roll_rad),
    })
}

/// Evaluates a producer-timed mode highlight without restarting it during redraw.
pub fn transition_visible(data: &PanelData, duration_ms: f32) -> bool {
    let flight = &data.flight;
    let transition = flight.guidance.value.transition;
    flight.guidance_present
        && flight.guidance.status == SignalStatus::Valid
        && duration_ms.is_finite()
        && duration_ms > 0.0
        && transition.reason != TransitionReason::None
        && transition
            .elapsed_ms
            .zip(flight.guidance_age_ms)
            .is_some_and(|(elapsed, age)| {
                elapsed >= 0.0 && age >= 0.0 && elapsed + age < duration_ms
            })
}

/// Returns the effective native target only while speed control is active.
pub fn effective_speed(data: &PanelData) -> Option<SpeedValue> {
    let flight = &data.flight;
    let sample = flight.guidance.value;
    if !flight.guidance_present
        || flight.guidance.status != SignalStatus::Valid
        || matches!(
            sample.speed_control,
            SpeedControl::None | SpeedControl::Unknown
        )
    {
        return None;
    }
    sample.speed_target.filter(|target| target.is_valid())
}

/// Returns a projection only when its computation and native target match active guidance.
pub fn effective_speed_projection(data: &PanelData) -> Option<ProjectedTarget> {
    let target = effective_speed(data)?;
    let flight = &data.flight;
    let speed = flight.speed.value;
    if !flight.speed_present
        || flight.speed.status != SignalStatus::Valid
        || speed.guidance_identity != flight.guidance.value.identity
        || !speed.guidance_identity.is_declared()
    {
        return None;
    }
    speed
        .effective
        .filter(|projected| projected.native == target && target_valid(*projected))
}

/// Current speed in display units with explicit coordinate and attribution.
pub fn current_speed(data: &PanelData) -> (SpeedCoordinate, Sig<f32>, GroupId) {
    if !data.flight.speed_present {
        return (SpeedCoordinate::Ias, data.ias_kt, GroupId::Air);
    }
    let speed = data.flight.speed;
    let current = speed.value.current.map_or_else(Sig::missing, |value| {
        let value = speed.value.coordinate.display_value(value);
        Sig::with_status(
            value,
            if value.is_finite() {
                speed.status
            } else {
                SignalStatus::Failed
            },
        )
    });
    (speed.value.coordinate, current, GroupId::SpeedPresentation)
}

/// Selected target remains distinct from the effective target.
pub fn selected_speed(data: &PanelData) -> Option<ProjectedTarget> {
    if data.flight.speed_present {
        return (data.flight.speed.status == SignalStatus::Valid)
            .then_some(data.flight.speed.value.selected)
            .flatten()
            .filter(|target| target_valid(*target));
    }
    let legacy = data.flight.ap_targets.airspeed_kt;
    (legacy.status == SignalStatus::Valid && legacy.value.is_finite() && legacy.value >= 0.0)
        .then_some(ProjectedTarget {
            native: SpeedValue {
                coordinate: SpeedCoordinate::Ias,
                value: legacy.value / crate::units::MPS_TO_KT,
            },
            projected: Some(legacy.value / crate::units::MPS_TO_KT),
        })
}
