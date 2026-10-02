//! The wind estimate, which stands apart from the flight sources.
//!
//! Wind is advisory and independently stamped, so it folds no source
//! trust: an estimate can stay usable while the sources behind an
//! attitude are not. A failed sample keeps its failure rather than
//! reading Missing, because a wind that broke and a wind nobody sent
//! are different situations for the reader.

use crate::aircraft::{AircraftState, Wind};
use crate::signal::{FreshnessPolicy, Sig, SignalStatus};
use crate::validate::StateIntegrity;

use super::fault_status;

pub(super) fn wind_signal(
    state: &AircraftState,
    policy: &FreshnessPolicy,
    integrity: &StateIntegrity,
) -> Sig<Wind> {
    let wind_status = policy
        .status_for_age(state.wind.age_ms)
        .worst(fault_status(integrity.wind));
    match (state.wind.data, wind_status) {
        (Some(w), s) if s.shows_value() => Sig::with_status(w, s),
        _ => Sig::with_status(
            Wind {
                from_rad: 0.0,
                speed_mps: 0.0,
            },
            if state.wind.data.is_some() && wind_status == SignalStatus::Failed {
                SignalStatus::Failed
            } else {
                SignalStatus::Missing
            },
        ),
    }
}
