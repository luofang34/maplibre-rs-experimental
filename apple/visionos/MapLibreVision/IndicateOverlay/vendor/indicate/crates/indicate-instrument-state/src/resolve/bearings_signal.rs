//! Bearing pointers, converted into the rose's reference.
//!
//! Each pointer carries the north its own receiver measured against, so
//! the pair is converted one at a time: one pointer can reach the rose
//! while the other cannot, and the one that cannot must lose its angle
//! rather than borrow its neighbour's.

use crate::aircraft::{AircraftState, BearingPointers};
use crate::heading::HeadingReference;
use crate::signal::{FreshnessPolicy, Sig, SignalStatus};

use super::finite;
use super::heading_signal::presented_angle;

/// The pointers and their rose-frame angles, under one group status.
///
/// The group status gates both: a group the state does not trust makes
/// every needle unusable, whatever the individual pointers declare. The
/// per-pointer `valid` flag is the second gate, and it belongs to the
/// consumer that draws the needle.
pub(super) fn bearings_resolved(
    state: &AircraftState,
    policy: &FreshnessPolicy,
    rose: HeadingReference,
    status: SignalStatus,
) -> (Sig<BearingPointers>, [Sig<f32>; 2]) {
    let pointers = state.bearings.data.unwrap_or_default();
    let rose_rad = [&pointers.first, &pointers.second].map(|pointer| {
        finite(presented_angle(
            Sig::with_status(pointer.bearing_rad, status),
            pointer.reference,
            rose,
            state,
            policy,
        ))
    });
    (Sig::with_status(pointers, status), rose_rad)
}
