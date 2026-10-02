//! The signals that come from equipment rather than from the
//! aircraft's motion, resolved together because they share a datum.
//!
//! The altitude declaration is the shared part: the pilot's selected
//! altitude and the automation's altitude target are both read against
//! it, and both are withheld when their reference identity does not
//! match it. Resolving them apart would mean resolving the altitude
//! twice, and two answers to that is one too many.

use crate::aircraft::{AircraftState, BearingPointers};
use crate::autopilot::ApModes;
use crate::group_id::{GroupId, GroupStatuses};
use crate::heading::HeadingReference;
use crate::signal::{FreshnessPolicy, Sig};
use crate::validate::StateIntegrity;

use super::altitude_signal::altitude_resolved;
use super::autoflight_signal::ap_targets_resolved;
use super::bearings_signal::bearings_resolved;
use super::kinematics_signal::KinematicSignals;
use super::{ApTargetsResolved, ResolvedAltitude, Trust};

/// What the equipment reports, in the rose's reference and the display's
/// units.
pub(super) struct EquipmentSignals {
    /// The datum-qualified altitude, and the identity every value read
    /// against it must match.
    pub altitude: ResolvedAltitude,
    /// Each pointer's bearing converted into the rose's reference.
    pub bearings_rose_rad: [Sig<f32>; 2],
    /// The pointers themselves, under one group status.
    pub bearings: Sig<BearingPointers>,
    /// Autoflight engagement and modes.
    pub ap_modes: Sig<ApModes>,
    /// The values the automation is flying toward.
    pub ap_targets: ApTargetsResolved,
}

/// Resolves the altitude declaration first, then everything read
/// against it.
pub(super) fn equipment_resolved(
    state: &AircraftState,
    policy: &FreshnessPolicy,
    trust: &Trust,
    integrity: &StateIntegrity,
    rose: HeadingReference,
    groups: &GroupStatuses,
    kin: &KinematicSignals,
) -> EquipmentSignals {
    let altitude = altitude_resolved(
        state,
        policy,
        trust,
        integrity,
        kin.position,
        kin.rel_alt_ft,
    );
    let (bearings, bearings_rose_rad) =
        bearings_resolved(state, policy, rose, groups.status(GroupId::BearingPointers));
    EquipmentSignals {
        ap_modes: Sig::with_status(
            state.ap_modes.data.unwrap_or_default(),
            groups.status(GroupId::ApModes),
        ),
        ap_targets: ap_targets_resolved(state, &altitude, groups.status(GroupId::ApTargets)),
        altitude,
        bearings_rose_rad,
        bearings,
    }
}
