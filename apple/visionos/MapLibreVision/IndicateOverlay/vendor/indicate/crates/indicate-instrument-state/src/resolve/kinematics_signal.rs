//! Kinematic geometry and the per-axis velocity statuses.
//!
//! Horizontal and vertical velocity are declared and validated
//! independently, so ground speed and track fold the horizontal status
//! while vertical speed folds the vertical one. Neither axis may borrow
//! the other's trust: a vertical speed shown on the strength of a
//! horizontal solution is a fabricated primary-flight reading, and a
//! horizontal solution withdrawn for want of a vertical estimate loses
//! two readings to flag a third.

use libm::{atan2f, sqrtf};

use crate::aircraft::AircraftState;
use crate::signal::{FreshnessPolicy, Sig, SignalStatus};
use crate::units::{M_TO_FT, MPS_TO_FPM, MPS_TO_KT};
use crate::validate::StateIntegrity;

use super::{Trust, group_freshness};

/// Below this groundspeed the track angle is geometrically meaningless
/// and resolves `Missing` instead of jittering.
const TRACK_MIN_GS_MPS: f32 = 0.5;

/// What one NED position/velocity sample yields, each quantity already
/// carrying the status of the axis it is read from.
pub(super) struct KinematicSignals {
    /// Height above the local origin in feet, behind the position
    /// status — the local-relative altitude class reads it.
    pub(super) rel_alt_ft: f32,
    /// Position status, shared by every position-derived quantity.
    pub(super) position: SignalStatus,
    /// Groundspeed in knots.
    pub(super) gs_kt: Sig<f32>,
    /// Vertical speed in feet/minute, positive climbing.
    pub(super) vsi_fpm: Sig<f32>,
    /// Ground track in radians clockwise from north, before conversion
    /// into the rose reference.
    pub(super) track_rad: Sig<f32>,
}

/// Resolves the kinematics group into its derived signals.
pub(super) fn kinematic_signals(
    state: &AircraftState,
    policy: &FreshnessPolicy,
    trust: &Trust,
    integrity: &StateIntegrity,
) -> KinematicSignals {
    let has = state.kinematics.data.is_some();
    let fresh = group_freshness(policy, has, state.kinematics.age_ms);
    let horizontal = trust.fold(
        has,
        fresh,
        integrity.velocity_horizontal,
        state.valid.velocity_horizontal,
    );
    let vertical = trust.fold(
        has,
        fresh,
        integrity.velocity_vertical,
        state.valid.velocity_vertical,
    );
    let geometry = geometry(state);
    // Track needs a horizontal solution and enough of one for the angle
    // to mean anything.
    let track = if geometry.gs_mps.is_finite() && geometry.gs_mps >= TRACK_MIN_GS_MPS {
        horizontal
    } else {
        SignalStatus::Missing
    };
    KinematicSignals {
        rel_alt_ft: geometry.rel_alt_ft,
        position: trust.fold(has, fresh, integrity.position, state.valid.position),
        gs_kt: Sig::with_status(geometry.gs_mps * MPS_TO_KT, horizontal),
        vsi_fpm: Sig::with_status(geometry.vsi_fpm, vertical),
        track_rad: Sig::with_status(geometry.track_rad, track),
    }
}

/// The raw geometry, before any status is attached. Quiet zeros stand
/// in for an absent group; the statuses above keep them off the glass.
struct Geometry {
    rel_alt_ft: f32,
    vsi_fpm: f32,
    gs_mps: f32,
    track_rad: f32,
}

fn geometry(state: &AircraftState) -> Geometry {
    match state.kinematics.data {
        Some(kin) => {
            let [north, east, down] = kin.vel_ned_mps;
            Geometry {
                rel_alt_ft: -kin.pos_ned_m[2] * M_TO_FT,
                vsi_fpm: -down * MPS_TO_FPM,
                gs_mps: sqrtf(north * north + east * east),
                track_rad: atan2f(east, north),
            }
        }
        None => Geometry {
            rel_alt_ft: 0.0,
            vsi_fpm: 0.0,
            gs_mps: 0.0,
            track_rad: 0.0,
        },
    }
}

#[cfg(test)]
mod tests;
