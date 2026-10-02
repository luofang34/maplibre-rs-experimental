//! Pure numeric/integrity validators applied before display resolution.
//!
//! No non-finite, malformed, unknown-quality, invalid-quaternion, or
//! otherwise untrusted value may resolve `Valid` or influence display
//! geometry. Validators never repair silently: a fault fails its group
//! with a typed reason, and independent group faults stay isolated.

use libm::sqrtf;

use crate::aircraft::{
    AircraftState, EstimateQuality, NavFromTo, NavSource, Selections, SnapshotCoherence,
};
use indicate_frames::Quat;

/// Largest relative quaternion norm error normalized instead of failed.
///
/// Within the tolerance the quaternion is renormalized (accumulated
/// rounding from an estimator is expected); at zero, gross, or
/// non-finite norm the attitude is unusable and fails instead.
pub const QUAT_NORM_TOLERANCE: f32 = 0.02;

/// Why one group's data cannot be trusted this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupFault {
    /// A mandatory value is NaN or infinite.
    NonFinite,
    /// The attitude quaternion has zero, gross, or non-finite norm.
    QuatNorm,
    /// The source declared a quality level this build does not know.
    UnknownQuality,
    /// An enum field carried a value this build does not know.
    UnknownEnum,
    /// A reference class requires a source sample, applied setting, or
    /// model identity that was not provided. The group fails; nothing
    /// substitutes.
    SourceAbsent,
    /// A bounded text field carried malformed wire content (over-length,
    /// out-of-charset, or non-canonical padding). Text nobody vetted
    /// must not display, so the group fails.
    MalformedIdent,
}

/// Per-group validation results; `None` means the group's received data
/// passed. Groups without data are not validated (absence is `Missing`,
/// not a fault).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StateIntegrity {
    /// Extended guidance identity, mode, or command fault.
    pub guidance: Option<GroupFault>,
    /// Extended measured speed fault.
    pub speed: Option<GroupFault>,
    /// Conditional envelope fault, independent of measured speed.
    pub envelope: Option<GroupFault>,
    /// Attitude quaternion fault.
    pub attitude: Option<GroupFault>,
    /// Body-rates fault.
    pub rates: Option<GroupFault>,
    /// NED position fault.
    pub position: Option<GroupFault>,
    /// North/east velocity fault; ground speed and track fold this.
    pub velocity_horizontal: Option<GroupFault>,
    /// Down velocity fault; vertical speed folds this. A non-finite
    /// down component faults here alone, leaving a horizontal solution
    /// the source really supplies untouched.
    pub velocity_vertical: Option<GroupFault>,
    /// Air-data fault.
    pub air: Option<GroupFault>,
    /// Navigation-guidance fault.
    pub nav: Option<GroupFault>,
    /// Wind-estimate fault.
    pub wind: Option<GroupFault>,
    /// Pilot-selections fault.
    pub selections: Option<GroupFault>,
    /// Source quality was undeclared or unknown (taints every estimate
    /// group, matching how quality itself combines).
    pub quality: Option<GroupFault>,
    /// Snapshot coherence carried an unknown wire value.
    pub coherence: Option<GroupFault>,
    /// Datum-qualified altitude fault: unknown reference class, missing
    /// required source, undeclared model, or non-finite sample.
    pub altitude: Option<GroupFault>,
    /// Heading-sample fault: non-finite value or unknown reference.
    pub heading: Option<GroupFault>,
    /// Magnetic-variation fault: non-finite value or undeclared source.
    pub variation: Option<GroupFault>,
    /// Dynamics fault: non-finite turn rate, lateral force, or airspeed
    /// trend, or an unknown turn basis.
    pub dynamics: Option<GroupFault>,
    /// Monitor-text fault: an impossible line count or malformed line
    /// content on the wire.
    pub monitor_text: Option<GroupFault>,
    /// Flight-director fault: non-finite or out-of-envelope commanded
    /// attitude, or an unknown mode/engagement byte.
    pub director: Option<GroupFault>,
    /// Bearing-pointer fault: an unknown source or reference, or a
    /// non-finite bearing.
    pub bearings: Option<GroupFault>,
    /// Airframe-configuration fault: a non-finite ratio, or one outside
    /// the range its axis is defined over.
    pub airframe: Option<GroupFault>,
    /// Autoflight-mode fault: a mode or engagement byte this build
    /// cannot name.
    pub ap_modes: Option<GroupFault>,
    /// Autoflight-target fault: a target present but not a number.
    pub ap_targets: Option<GroupFault>,
}

fn all_finite(values: &[f32]) -> bool {
    values.iter().all(|value| value.is_finite())
}

fn opt_finite(value: Option<f32>) -> bool {
    value.is_none_or(f32::is_finite)
}

/// Validates the attitude quaternion: every component finite and the
/// norm within [`QUAT_NORM_TOLERANCE`] of unity. Returns the normalized
/// quaternion; never repairs a gross error.
pub fn validate_quat(quat: Quat) -> Result<Quat, GroupFault> {
    if !all_finite(&[quat.w, quat.x, quat.y, quat.z]) {
        return Err(GroupFault::NonFinite);
    }
    let norm = sqrtf(quat.w * quat.w + quat.x * quat.x + quat.y * quat.y + quat.z * quat.z);
    if !norm.is_finite() || (norm - 1.0).abs() > QUAT_NORM_TOLERANCE {
        return Err(GroupFault::QuatNorm);
    }
    Ok(Quat {
        w: quat.w / norm,
        x: quat.x / norm,
        y: quat.y / norm,
        z: quat.z / norm,
    })
}

fn selections_fault(selections: &Selections) -> Option<GroupFault> {
    let finite = selections.heading_bug_rad.is_finite()
        && opt_finite(selections.altitude_sel_m)
        && opt_finite(selections.baro_sel_hpa);
    if finite {
        None
    } else {
        Some(GroupFault::NonFinite)
    }
}

/// The nav group's own fault, if it has one.
///
/// An unknown scale fails with the other unknown enumerations: the
/// deflection is in dots, and a dot means nothing until the scale says
/// what it is worth.
fn nav_fault(nav: &crate::aircraft::NavData) -> Option<GroupFault> {
    if matches!(nav.source, NavSource::Unknown)
        || matches!(nav.fromto, NavFromTo::Unknown)
        || matches!(nav.scale, crate::aircraft::NavScale::Unknown)
    {
        return Some(GroupFault::UnknownEnum);
    }
    if nav.to_ident.is_invalid() || nav.from_ident.is_invalid() {
        return Some(GroupFault::MalformedIdent);
    }
    if !(all_finite(&[nav.course_rad, nav.cdi_dots])
        && opt_finite(nav.vdev_dots)
        && opt_finite(nav.dist_nm))
    {
        return Some(GroupFault::NonFinite);
    }
    None
}

/// The bearing group's own fault, if it has one.
///
/// A pointer whose source this build cannot name, or whose north it
/// cannot resolve, fails the group: a needle pointing somewhere on
/// behalf of nobody is worse than no needle. A pointer the source
/// declares unusable is not a fault — it is simply not drawn.
fn bearings_fault(pointers: &crate::aircraft::BearingPointers) -> Option<GroupFault> {
    for pointer in [&pointers.first, &pointers.second] {
        if matches!(pointer.source, crate::aircraft::NavSource::Unknown)
            || matches!(pointer.reference, crate::heading::HeadingReference::Unknown)
        {
            return Some(GroupFault::UnknownEnum);
        }
        if pointer.valid && !pointer.bearing_rad.is_finite() {
            return Some(GroupFault::NonFinite);
        }
    }
    None
}

/// The airframe group's own fault, if it has one.
///
/// A ratio outside the range its axis is defined over is not a reading
/// the display can place on a scale, so it faults rather than clamping:
/// a clamped pointer would sit at a limit the airframe never reached.
fn airframe_fault(config: &crate::aircraft::AirframeConfig) -> Option<GroupFault> {
    let in_unit = |value: Option<f32>| match value {
        Some(v) => v.is_finite() && (0.0..=1.0).contains(&v),
        None => true,
    };
    let in_signed = |value: Option<f32>| match value {
        Some(v) => v.is_finite() && (-1.0..=1.0).contains(&v),
        None => true,
    };
    if in_unit(config.flap_ratio)
        && in_unit(config.flap_selected_ratio)
        && in_signed(config.elevator_trim_ratio)
        && in_signed(config.aileron_trim_ratio)
        && in_signed(config.rudder_trim_ratio)
    {
        None
    } else {
        Some(GroupFault::NonFinite)
    }
}

/// The two groups whose members fault separately: attitude reports its
/// quaternion and its rates apart, and kinematics reports position and
/// each velocity axis apart, because a consumer of one is not
/// necessarily a consumer of the others.
fn validate_state_estimates(state: &AircraftState, integrity: &mut StateIntegrity) {
    if let Some(attitude) = &state.attitude.data {
        if let Err(fault) = validate_quat(attitude.quat) {
            integrity.attitude = Some(fault);
        }
        if !all_finite(&attitude.rates_rps) {
            integrity.rates = Some(GroupFault::NonFinite);
        }
    }
    if let Some(kinematics) = &state.kinematics.data {
        if !all_finite(&kinematics.pos_ned_m) {
            integrity.position = Some(GroupFault::NonFinite);
        }
        let [vel_north, vel_east, vel_down] = kinematics.vel_ned_mps;
        if !all_finite(&[vel_north, vel_east]) {
            integrity.velocity_horizontal = Some(GroupFault::NonFinite);
        }
        if !vel_down.is_finite() {
            integrity.velocity_vertical = Some(GroupFault::NonFinite);
        }
    }
}

/// Validates every received group of `state` and reports per-group
/// faults. Absent groups pass (their absence resolves `Missing`); the
/// deterministic worst-of combination in `resolve` folds these faults
/// into each signal's status.
pub fn validate_state(state: &AircraftState) -> StateIntegrity {
    let mut integrity = StateIntegrity {
        guidance: state
            .flight
            .guidance
            .data
            .as_ref()
            .and_then(crate::flight::guidance_fault),
        speed: state
            .flight
            .speed
            .data
            .as_ref()
            .and_then(crate::flight::speed_fault),
        envelope: state
            .flight
            .speed
            .data
            .as_ref()
            .and_then(crate::flight::speed_envelope_fault),
        ..StateIntegrity::default()
    };
    validate_state_estimates(state, &mut integrity);

    if let Some(air) = &state.air.data
        && !(opt_finite(air.ias_mps) && opt_finite(air.baro_setting_hpa) && opt_finite(air.tas_mps))
    {
        integrity.air = Some(GroupFault::NonFinite);
    }
    if let Some(pointers) = &state.bearings.data {
        integrity.bearings = bearings_fault(pointers);
    }
    if let Some(config) = &state.airframe.data {
        integrity.airframe = airframe_fault(config);
    }
    if let Some(nav) = &state.nav.data {
        integrity.nav = nav_fault(nav);
    }
    if let Some(wind) = &state.wind.data
        && !all_finite(&[wind.from_rad, wind.speed_mps])
    {
        integrity.wind = Some(GroupFault::NonFinite);
    }
    integrity.selections = selections_fault(&state.selections);
    if state.quality == EstimateQuality::Unknown {
        integrity.quality = Some(GroupFault::UnknownQuality);
    }
    if state.snapshot.coherence == SnapshotCoherence::Unknown {
        integrity.coherence = Some(GroupFault::UnknownEnum);
    }
    integrity.altitude = altitude_fault(state);
    if let Some(heading) = &state.heading.data {
        if heading.reference == crate::heading::HeadingReference::Unknown {
            integrity.heading = Some(GroupFault::UnknownEnum);
        } else if !heading.heading_rad.is_finite() {
            integrity.heading = Some(GroupFault::NonFinite);
        }
    }
    if let Some(variation) = &state.variation.data {
        if !variation.east_positive_rad.is_finite() {
            integrity.variation = Some(GroupFault::NonFinite);
        } else if variation.source == crate::heading::VariationSourceId::UNDECLARED {
            integrity.variation = Some(GroupFault::SourceAbsent);
        }
    }
    integrity.dynamics = state.dynamics.data.as_ref().and_then(dynamics_fault);
    integrity.director = state.director.data.as_ref().and_then(director_fault);
    integrity.ap_modes = state.ap_modes.data.as_ref().and_then(ap_modes_fault);
    integrity.ap_targets = ap_targets_fault(&state.ap_targets);
    if let Some(text) = &state.monitor_text.data
        && text.is_malformed()
    {
        integrity.monitor_text = Some(GroupFault::MalformedIdent);
    }
    integrity
}

/// A mode this build cannot name fails the whole group. Annunciating
/// the modes it could read beside a silence where the unreadable one
/// belongs would say the automation holds nothing on that axis, which
/// is a claim nobody made.
fn ap_modes_fault(modes: &crate::autopilot::ApModes) -> Option<GroupFault> {
    use crate::autopilot::{ApEngagement, LateralMode, VerticalMode};
    let unknown = modes.engagement == ApEngagement::Unknown
        || modes.lateral_active == LateralMode::Unknown
        || modes.lateral_armed == LateralMode::Unknown
        || modes.vertical_active == VerticalMode::Unknown
        || modes.vertical_armed == VerticalMode::Unknown;
    unknown.then_some(GroupFault::UnknownEnum)
}

/// A target the automation is flying toward has to be a number. The
/// reference identity of the altitude target is not checked here: an
/// identity that does not match the displayed declaration is a
/// comparability question the resolver answers by withholding the
/// readout, not a fault in the source.
fn ap_targets_fault(targets: &crate::autopilot::ApTargets) -> Option<GroupFault> {
    let finite = opt_finite(targets.airspeed_mps)
        && opt_finite(targets.vertical_speed_mps)
        && opt_finite(targets.altitude_m);
    (!finite).then_some(GroupFault::NonFinite)
}

fn director_fault(director: &crate::director::FdSample) -> Option<GroupFault> {
    use crate::director::{FdEngagement, FdMode};
    if director.mode == FdMode::Unknown || director.engagement == FdEngagement::Unknown {
        return Some(GroupFault::UnknownEnum);
    }
    let pitch_ok = director.pitch_cmd_rad.is_finite()
        && director.pitch_cmd_rad.abs() <= core::f32::consts::FRAC_PI_2;
    let roll_ok =
        director.roll_cmd_rad.is_finite() && director.roll_cmd_rad.abs() <= core::f32::consts::PI;
    if !(pitch_ok && roll_ok) {
        return Some(GroupFault::NonFinite);
    }
    None
}

fn dynamics_fault(dynamics: &crate::dynamics::DynSample) -> Option<GroupFault> {
    let unknown_basis = dynamics
        .turn
        .is_some_and(|sample| sample.basis == crate::dynamics::TurnBasis::Unknown);
    if unknown_basis {
        return Some(GroupFault::UnknownEnum);
    }
    let turn_bad = dynamics
        .turn
        .is_some_and(|sample| !sample.rate_rps.is_finite());
    if turn_bad || !opt_finite(dynamics.lateral_mps2) || !opt_finite(dynamics.ias_trend_mps2) {
        return Some(GroupFault::NonFinite);
    }
    None
}

/// Typed reason a datum-qualified altitude cannot display. Class rules:
/// local-relative needs no sample; barometric indicated needs the sample
/// and the source-applied setting; pressure, geometric MSL, and AGL need
/// the sample; geometric MSL also needs a declared model. An unknown
/// class or a non-finite sample fails outright — no reference is ever
/// guessed and no fallback is ever taken.
fn altitude_fault(state: &AircraftState) -> Option<GroupFault> {
    use crate::altitude::{AltitudeClass, GeoidModelId};
    let decl = &state.altitude;
    if !opt_finite(decl.sample_m) {
        return Some(GroupFault::NonFinite);
    }
    let applied = state.air.data.and_then(|air| air.baro_setting_hpa);
    match decl.reference_class {
        AltitudeClass::LocalRelative => None,
        AltitudeClass::BaroIndicated => {
            if decl.sample_m.is_none() || applied.is_none() {
                Some(GroupFault::SourceAbsent)
            } else {
                None
            }
        }
        AltitudeClass::Pressure | AltitudeClass::Agl => {
            if decl.sample_m.is_none() {
                Some(GroupFault::SourceAbsent)
            } else {
                None
            }
        }
        AltitudeClass::GeometricMsl => {
            if decl.sample_m.is_none() || decl.geoid_model == GeoidModelId::UNDECLARED {
                Some(GroupFault::SourceAbsent)
            } else {
                None
            }
        }
        AltitudeClass::Unknown => Some(GroupFault::UnknownEnum),
    }
}

#[cfg(test)]
mod tests;
