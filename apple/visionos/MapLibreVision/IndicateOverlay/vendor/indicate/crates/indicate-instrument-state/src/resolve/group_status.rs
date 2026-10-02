//! Group-level status reporting: the generic per-[`GroupId`] surface a
//! registry or admission harness asks, instead of a method per group.
//!
//! Each group's status is the group-level worst-of over the same inputs
//! its rendered signals fold — freshness, source trust, per-group
//! validation, declared validity. A group with several members (both
//! kinematic vectors, every dynamics sample) reports the worst member,
//! so this surface can only be more conservative than any one signal.

use crate::aircraft::AircraftState;
use crate::group_id::{GroupId, GroupStatuses};
use crate::signal::{FreshnessPolicy, SignalStatus};
use crate::validate::StateIntegrity;

use super::{Trust, fault_status, group_freshness};

/// The monitor channel's own slow policy: a live machine feed updates
/// irregularly and must not flap under the flight-data thresholds.
pub(crate) const TEXT_FRESHNESS: FreshnessPolicy =
    FreshnessPolicy::from_validated_literals(2000.0, 10_000.0);

pub(super) fn group_statuses(
    state: &AircraftState,
    policy: &FreshnessPolicy,
    trust: &Trust,
    integrity: &StateIntegrity,
) -> GroupStatuses {
    let mut out = GroupStatuses::default();
    for id in GroupId::ALL {
        out.set(id, group_status(state, policy, trust, integrity, id));
    }
    out
}

/// The presence and freshness inputs one stamped group contributes to
/// its fold.
struct Ctx<'a> {
    policy: &'a FreshnessPolicy,
    trust: &'a Trust,
    has: bool,
    age_ms: Option<f32>,
}

impl<'a> Ctx<'a> {
    fn of<T>(
        policy: &'a FreshnessPolicy,
        trust: &'a Trust,
        stamped: &crate::aircraft::Stamped<T>,
    ) -> Self {
        Self {
            policy,
            trust,
            has: stamped.data.is_some(),
            age_ms: stamped.age_ms,
        }
    }
}

/// One group's trust fold from its presence, freshness, integrity, and
/// declared validity.
fn fold(ctx: &Ctx<'_>, fault: Option<crate::validate::GroupFault>, valid: bool) -> SignalStatus {
    let fresh = group_freshness(ctx.policy, ctx.has, ctx.age_ms);
    ctx.trust.fold(ctx.has, fresh, fault, valid)
}

/// Attitude carries the rates alongside the quaternion, so one fault or
/// one cleared validity bit in either takes the whole group down: a
/// horizon drawn from a trusted quaternion and untrusted rates is still
/// a horizon nobody vouched for.
fn attitude_status(
    state: &AircraftState,
    policy: &FreshnessPolicy,
    trust: &Trust,
    integrity: &StateIntegrity,
) -> SignalStatus {
    fold(
        &Ctx {
            policy,
            trust,
            has: state.attitude.data.is_some(),
            age_ms: state.attitude.age_ms,
        },
        integrity.attitude.or(integrity.rates),
        state.valid.attitude && state.valid.rates,
    )
}

/// Position and both velocity axes share one group, so the group is no
/// better than its worst member.
fn kinematics_status(
    state: &AircraftState,
    policy: &FreshnessPolicy,
    trust: &Trust,
    integrity: &StateIntegrity,
) -> SignalStatus {
    fold(
        &Ctx {
            policy,
            trust,
            has: state.kinematics.data.is_some(),
            age_ms: state.kinematics.age_ms,
        },
        integrity
            .position
            .or(integrity.velocity_horizontal)
            .or(integrity.velocity_vertical),
        state.valid.position && state.valid.velocity_horizontal && state.valid.velocity_vertical,
    )
}

/// Wind folds no source trust, mirroring its resolved signal: a wind
/// estimate is advisory and independently stamped, so it can stay
/// usable while the sources behind an attitude are not.
fn wind_status(
    state: &AircraftState,
    policy: &FreshnessPolicy,
    integrity: &StateIntegrity,
) -> SignalStatus {
    group_freshness(policy, state.wind.data.is_some(), state.wind.age_ms)
        .worst(fault_status(integrity.wind))
}

fn group_status(
    state: &AircraftState,
    policy: &FreshnessPolicy,
    trust: &Trust,
    integrity: &StateIntegrity,
    id: GroupId,
) -> SignalStatus {
    match id {
        GroupId::Guidance | GroupId::SpeedPresentation => {
            flight_status(state, policy, trust, integrity, id)
        }
        GroupId::Attitude => attitude_status(state, policy, trust, integrity),
        GroupId::Kinematics => kinematics_status(state, policy, trust, integrity),
        GroupId::Air => fold(&Ctx::of(policy, trust, &state.air), integrity.air, true),
        GroupId::Nav => fold(&Ctx::of(policy, trust, &state.nav), integrity.nav, true),
        GroupId::Wind => wind_status(state, policy, integrity),
        GroupId::Selections => fault_status(integrity.selections),
        // Absent trust is fail-closed Failed, never Missing: trust must
        // be declared before any estimate group can show Valid.
        GroupId::Trust => trust.quality.worst(trust.coherence),
        GroupId::Altitude => fault_status(integrity.altitude),
        GroupId::Heading => fold(
            &Ctx::of(policy, trust, &state.heading),
            integrity.heading,
            state.valid.heading,
        ),
        GroupId::Variation => fold(
            &Ctx::of(policy, trust, &state.variation),
            integrity.variation,
            state.valid.variation,
        ),
        GroupId::Dynamics => fold(
            &Ctx::of(policy, trust, &state.dynamics),
            integrity.dynamics,
            state.valid.turn && state.valid.slip && state.valid.ias_trend,
        ),
        // Bearings fold source trust like nav: a needle from a source
        // the state does not trust must not point anywhere. Their own
        // per-pointer validity is a second gate the panel applies.
        GroupId::BearingPointers => fold(
            &Ctx::of(policy, trust, &state.bearings),
            integrity.bearings,
            true,
        ),
        // Configuration folds source trust like the rest: a position
        // reported by a source the state does not trust is not a
        // position. It declares no validity bit of its own — the trust
        // group's bits cover sensed estimates, and a flap setting is a
        // reading rather than an estimate.
        GroupId::AirframeConfig => fold(
            &Ctx::of(policy, trust, &state.airframe),
            integrity.airframe,
            true,
        ),
        // Autoflight modes fold source trust like the director: an
        // annunciation sourced from a state nobody trusts must not say
        // the automation is holding something.
        GroupId::ApModes => fold(
            &Ctx::of(policy, trust, &state.ap_modes),
            integrity.ap_modes,
            true,
        ),
        // Targets fold no source trust, mirroring selections: a target
        // is UI state, and it carries no age to grow stale.
        GroupId::ApTargets => fault_status(integrity.ap_targets),
        // The director folds source trust like nav: a command from an
        // untrusted source must not draw bars.
        GroupId::FlightDirector => fold(
            &Ctx::of(policy, trust, &state.director),
            integrity.director,
            true,
        ),
        // Advisory machine text folds no flight-source trust and runs
        // its own slow freshness policy, mirroring wind's independence.
        GroupId::MonitorText => monitor_status(state, integrity),
    }
}

fn monitor_status(state: &AircraftState, integrity: &StateIntegrity) -> SignalStatus {
    group_freshness(
        &TEXT_FRESHNESS,
        state.monitor_text.data.is_some(),
        state.monitor_text.age_ms,
    )
    .worst(fault_status(integrity.monitor_text))
}

fn flight_status(
    state: &AircraftState,
    policy: &FreshnessPolicy,
    trust: &Trust,
    integrity: &StateIntegrity,
    id: GroupId,
) -> SignalStatus {
    if id == GroupId::Guidance {
        fold(
            &Ctx::of(policy, trust, &state.flight.guidance),
            integrity.guidance,
            true,
        )
    } else {
        fold(
            &Ctx::of(policy, trust, &state.flight.speed),
            integrity.speed,
            true,
        )
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::TEXT_FRESHNESS;
    use crate::signal::FreshnessPolicy;

    #[test]
    fn text_freshness_passes_the_validating_constructor() {
        let validated = FreshnessPolicy::new(
            TEXT_FRESHNESS.stale_after_ms(),
            TEXT_FRESHNESS.fail_after_ms(),
        )
        .expect("literal thresholds validate");
        assert_eq!(validated, TEXT_FRESHNESS);
    }
}
