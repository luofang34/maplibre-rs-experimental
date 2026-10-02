//! Resolution of the datum-qualified altitude and the full-identity
//! selection-compatibility rule (ALT-01).

use crate::aircraft::AircraftState;
use crate::altitude::AltitudeClass;
use crate::signal::{FreshnessPolicy, Sig, SignalStatus};
use crate::units::M_TO_FT;
use crate::validate::StateIntegrity;

use super::{
    BARO_SETTING_TOLERANCE_HPA, ResolvedAltitude, Trust, fault_status, finite, group_freshness,
};

/// Resolves the datum-qualified altitude for the declared class. The
/// non-local classes ride the air-data group's stamp in ABI v3 (they
/// arrive beside it); a dedicated source group would bring its own
/// stamp. A required source that is absent fails the altitude — the
/// value is a quiet zero and nothing substitutes.
pub(super) fn altitude_resolved(
    state: &AircraftState,
    policy: &FreshnessPolicy,
    trust: &Trust,
    integrity: &StateIntegrity,
    pos_status: SignalStatus,
    rel_alt_ft: f32,
) -> ResolvedAltitude {
    let decl = state.altitude;
    let class = decl.reference_class;
    let fault = fault_status(integrity.altitude);
    let sample_ft = decl.sample_m.map(|m| m * M_TO_FT);
    let sample_status = group_freshness(policy, state.air.data.is_some(), state.air.age_ms)
        .worst(trust.quality)
        .worst(trust.coherence)
        .worst(fault);
    let value = match class {
        AltitudeClass::LocalRelative => Sig::with_status(rel_alt_ft, pos_status.worst(fault)),
        AltitudeClass::BaroIndicated
        | AltitudeClass::Pressure
        | AltitudeClass::GeometricMsl
        | AltitudeClass::Agl => match (sample_ft, integrity.altitude) {
            (Some(v), None) => Sig::with_status(v, sample_status),
            _ => Sig::with_status(0.0, SignalStatus::Failed),
        },
        AltitudeClass::Unknown => Sig::with_status(0.0, SignalStatus::Failed),
    };
    let applied = state.air.data.and_then(|air| air.baro_setting_hpa);
    let setting_mismatch = class == AltitudeClass::BaroIndicated
        && matches!(
            (applied, state.selections.baro_sel_hpa),
            (Some(a), Some(s)) if (a - s).abs() > BARO_SETTING_TOLERANCE_HPA
        );
    let bug_compatible = selection_compatible(state, class, setting_mismatch);
    ResolvedAltitude {
        value_ft: finite(value),
        class,
        origin: decl.origin,
        setting_mismatch,
        bug_compatible,
    }
}

/// The complete reference identity of a value expressed against the
/// altitude datum. A class alone is never an identity, so the three
/// travel together and are compared together.
pub(super) struct AltitudeIdentity {
    /// The class the value is expressed in.
    pub class: AltitudeClass,
    /// Origin of a local-relative value.
    pub origin: crate::altitude::OriginId,
    /// Declared geoid model of a geometric-MSL value.
    pub model: crate::altitude::GeoidModelId,
    /// Whether the value exists at all.
    pub present: bool,
}

/// Whether a value shares the displayed datum's COMPLETE reference
/// identity — class equality alone is never compatibility.
/// Local-relative values must name the same origin; geometric-MSL
/// values must name the same declared model; a barometric value's datum
/// is the applied setting, so a disputed setting suppresses it;
/// pressure altitude's datum is fully identified by its class (standard
/// atmosphere); AGL carries no source identity in this ABI revision, so
/// class equality is its complete identity today. Anything unknown or
/// incomplete fails closed.
///
/// One rule serves every value drawn against the datum — the pilot's
/// selection and the automation's target both — so the two cannot drift
/// into disagreeing about what makes an identity complete.
pub(super) fn identity_compatible(
    identity: &AltitudeIdentity,
    state: &AircraftState,
    displayed: AltitudeClass,
    setting_mismatch: bool,
) -> bool {
    if !identity.present || identity.class != displayed {
        return false;
    }
    let decl = state.altitude;
    match displayed {
        AltitudeClass::LocalRelative => identity.origin == decl.origin,
        AltitudeClass::GeometricMsl => {
            identity.model == decl.geoid_model
                && identity.model != crate::altitude::GeoidModelId::UNDECLARED
        }
        AltitudeClass::BaroIndicated => !setting_mismatch,
        AltitudeClass::Pressure | AltitudeClass::Agl => true,
        AltitudeClass::Unknown => false,
    }
}

fn selection_compatible(
    state: &AircraftState,
    displayed: AltitudeClass,
    setting_mismatch: bool,
) -> bool {
    let selections = state.selections;
    identity_compatible(
        &AltitudeIdentity {
            class: selections.altitude_sel_class,
            origin: selections.altitude_sel_origin,
            model: selections.altitude_sel_model,
            present: selections.altitude_sel_m.is_some(),
        },
        state,
        displayed,
        setting_mismatch,
    )
}
