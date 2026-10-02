//! Autoflight modes and the targets the automation flies toward.
//!
//! The targets arrive in SI and are resolved into the units the readout
//! shows, so the panel converts nothing: a display that does arithmetic
//! on a target is a display that can disagree with the resolver about
//! what the target is.
//!
//! The altitude target is the one that can be present and still
//! unusable. It carries its own reference identity, and a target
//! expressed against a datum the display is not showing is not
//! comparable to the altitude beside it. Rather than draw a number the
//! pilot could read against the wrong datum, the readout goes Missing.

use crate::aircraft::AircraftState;
use crate::signal::{Sig, SignalStatus};
use crate::units::{M_TO_FT, MPS_TO_FPM, MPS_TO_KT};

use super::altitude_signal::{AltitudeIdentity, identity_compatible};
use super::finite;
use super::{ApTargetsResolved, ResolvedAltitude};

/// The targets in display units, each carrying whether it may be shown.
///
/// `status` is the group's own status; a target the group does not
/// carry resolves `Missing` on its own, so an automation holding
/// altitude does not grow an airspeed target it never reported.
pub(super) fn ap_targets_resolved(
    state: &AircraftState,
    altitude: &ResolvedAltitude,
    status: SignalStatus,
) -> ApTargetsResolved {
    let targets = state.ap_targets;
    let present = |value: Option<f32>, scale: f32| match value {
        Some(v) if status.shows_value() => finite(Sig::with_status(v * scale, status)),
        Some(_) => Sig::with_status(0.0, status),
        None => Sig::with_status(0.0, SignalStatus::Missing),
    };
    let comparable = identity_compatible(
        &AltitudeIdentity {
            class: targets.altitude_class,
            origin: targets.altitude_origin,
            model: targets.altitude_model,
            present: targets.altitude_m.is_some(),
        },
        state,
        altitude.class,
        altitude.setting_mismatch,
    );
    ApTargetsResolved {
        airspeed_kt: present(targets.airspeed_mps, MPS_TO_KT),
        vertical_speed_fpm: present(targets.vertical_speed_mps, MPS_TO_FPM),
        altitude_ft: if comparable {
            present(targets.altitude_m, M_TO_FT)
        } else {
            Sig::with_status(0.0, SignalStatus::Missing)
        },
    }
}
