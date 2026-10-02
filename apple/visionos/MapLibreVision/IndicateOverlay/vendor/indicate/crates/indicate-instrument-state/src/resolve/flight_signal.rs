//! Shared flight-state resolution keeps envelope failures independent of speed.

use super::{
    Trust, director_signal::director_resolved, equipment_signal::EquipmentSignals, fault_status,
};
use crate::flight::FlightDisplayData;
use crate::group_id::GroupStatuses;
use crate::{AircraftState, FreshnessPolicy, GroupId, Sig, StateIntegrity};

pub(super) fn flight_resolved(
    state: &AircraftState,
    policy: &FreshnessPolicy,
    trust: &Trust,
    integrity: &StateIntegrity,
    groups: &GroupStatuses,
    equipment: &EquipmentSignals,
) -> FlightDisplayData {
    let speed_status = groups.status(GroupId::SpeedPresentation);
    FlightDisplayData {
        director: director_resolved(state, policy, trust, integrity),
        ap_modes: equipment.ap_modes,
        ap_targets: equipment.ap_targets,
        guidance: Sig::with_status(
            state.flight.guidance.data.unwrap_or_default(),
            groups.status(GroupId::Guidance),
        ),
        speed: Sig::with_status(state.flight.speed.data.unwrap_or_default(), speed_status),
        envelope_status: speed_status.worst(fault_status(integrity.envelope)),
        guidance_present: state.flight.guidance.data.is_some(),
        speed_present: state.flight.speed.data.is_some(),
        guidance_age_ms: state.flight.guidance.age_ms,
    }
}
