//! Resolution from raw input state to display-ready signals.

use crate::aircraft::{
    AircraftState, EstimateQuality, NavData, NavSource, Selections, SnapshotCoherence, Wind,
};
use crate::altitude::{AltitudeClass, OriginId};
use crate::dynamics::TurnBasis;
use crate::presentation::{AirframeDisplayProfile, AttitudePresentation, UnusualAttitudeState};
use crate::signal::{FreshnessPolicy, Sig, SignalStatus};
use crate::units::MPS_TO_KT;
use crate::validate::{StateIntegrity, validate_quat, validate_state};

/// Resolved navigation guidance for the HSI.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavResolved {
    /// Guidance data as provided; `source == None` removes the CDI.
    pub data: NavData,
    /// Status of the guidance group as a whole.
    pub status: SignalStatus,
    /// Selected course presented in the rose reference; `Failed` when
    /// the course's own reference is unknown or cannot convert. The CDI
    /// and course box render only from this, never from the raw angle.
    pub course_rose_rad: Sig<f32>,
}

/// The pilot-selected and source-applied altimeter settings disagree
/// beyond this tolerance (hectopascals).
pub const BARO_SETTING_TOLERANCE_HPA: f32 = 0.5;

/// Datum-qualified altitude resolved for display (ALT-01): the value
/// never changes reference silently, a barometric class fails without
/// its source instead of falling back to local NED, and selection
/// compatibility is decided by class, never by numeric coincidence.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedAltitude {
    /// Displayed altitude in feet; quiet zero behind a hidden status.
    pub value_ft: Sig<f32>,
    /// Reference class, for the tape label and compatibility checks.
    pub class: AltitudeClass,
    /// Origin identity when the class is local-relative.
    pub origin: OriginId,
    /// Pilot-selected setting disagrees with the source-applied one.
    pub setting_mismatch: bool,
    /// The selected altitude shares the displayed reference class, so
    /// the bug and selection readout may render.
    pub bug_compatible: bool,
}

/// The autoflight targets in the units their readouts show.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ApTargetsResolved {
    /// Target indicated airspeed in knots.
    pub airspeed_kt: Sig<f32>,
    /// Target vertical speed in feet per minute, positive up.
    pub vertical_speed_fpm: Sig<f32>,
    /// Target altitude in feet, `Missing` unless its reference identity
    /// matches the displayed datum's.
    pub altitude_ft: Sig<f32>,
}

/// Turn indication resolved from the typed dynamics group (DYN-01):
/// the rate, its explicit basis, and nothing derived from body rates.
/// The value is retained unclamped for monitoring — only the pointer
/// geometry saturates at the display scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedTurn {
    /// Turn rate in radians/second, positive right; NEVER body yaw
    /// rate — an absent dynamics group resolves `Missing`.
    pub rate_rps: Sig<f32>,
    /// What the displayed rate measures.
    pub basis: TurnBasis,
}

/// Display-ready state consumed by every panel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelData {
    /// Bank angle, radians, positive right.
    pub roll_rad: Sig<f32>,
    /// Pitch angle, radians, positive nose-up.
    pub pitch_rad: Sig<f32>,
    /// Independent, reference-typed heading.
    pub heading: ResolvedHeading,
    /// What orients the rose and which reference the angular
    /// quantities present in.
    pub rose_basis: RoseBasis,
    /// Heading bug presented in the rose reference; `Failed` when the
    /// bug's own reference is unknown or cannot convert.
    pub heading_bug_rose_rad: Sig<f32>,
    /// Each bearing pointer converted into the rose reference, in draw
    /// order. A pointer whose north cannot be resolved carries the
    /// status of that failure, and the panel draws no needle for it.
    pub bearings_rose_rad: [Sig<f32>; 2],
    /// The bearing pointers as declared, with the group's status.
    pub bearings: Sig<crate::aircraft::BearingPointers>,
    /// Shared guidance, selected targets, and speed-presentation state.
    pub flight: crate::flight::FlightDisplayData,
    /// Typed turn indication; body rates never feed this.
    pub turn: ResolvedTurn,
    /// Lateral specific force (m/s², body +Y right) for the slip/skid
    /// ball; missing stays missing, never synthesized centered.
    pub slip_lat_mps2: Sig<f32>,
    /// Rate of change of indicated airspeed, knots per second. The
    /// trend cue reads it directly; the display never differences its
    /// own frames for it.
    pub ias_trend_kt_s: Sig<f32>,
    /// Indicated airspeed, knots.
    pub ias_kt: Sig<f32>,
    /// True airspeed, knots. Source-supplied only: the display never
    /// derives it from indicated airspeed (ADR-0017), so a source that
    /// supplies one Air field and not the other leaves this `Missing`.
    pub tas_kt: Sig<f32>,
    /// Groundspeed, knots.
    pub gs_kt: Sig<f32>,
    /// Datum-qualified altitude.
    pub altitude: ResolvedAltitude,
    /// Vertical speed, feet/minute, positive climbing.
    pub vsi_fpm: Sig<f32>,
    /// Ground track, radians clockwise from north.
    pub track_rad: Sig<f32>,
    /// Altimeter setting, hectopascals.
    pub baro_hpa: Sig<f32>,
    /// Wind estimate.
    pub wind: Sig<Wind>,
    /// Navigation guidance.
    pub nav: NavResolved,
    /// Pilot selections, sanitized: a non-finite selection is dropped to
    /// its neutral value and reported in `integrity`, never drawn raw.
    pub selections: Selections,
    /// Per-group typed fault reasons behind any validation-driven
    /// status downgrade, for annunciation and diagnostics.
    pub integrity: StateIntegrity,
    /// SO(3)-safe attitude presentation (tier, chevrons, declutter,
    /// continuous bank). Meaningful only while the attitude signals'
    /// status shows a value; an invalid attitude resets it to default.
    pub presentation: AttitudePresentation,
    /// Profile policy: a missing/failed turn or slip indication must
    /// show a visible failure cue (DYN-01).
    pub require_dynamics_cue: bool,
    /// Per-function selected source and reversion state (SRC-01). Default
    /// when the caller resolves without source comparison;
    /// [`crate::resolve_with_sources`] populates it.
    pub sources: crate::source_monitor::SourceSelection,
    /// Machine-monitoring text channel (AIR-IN-014), advisory only; a
    /// hidden status leaves the default empty channel behind it.
    pub monitor_text: Sig<crate::monitor_text::MonitorText>,
    /// Airframe configuration: flap position and trim, with the group's
    /// own status. Each ratio stays optional inside it — a vehicle with
    /// a flap sensor and no trim sensor shows one scale, not two.
    pub airframe: Sig<crate::aircraft::AirframeConfig>,
    /// Group-level status keyed by [`crate::GroupId`] — the surface a
    /// registry or admission harness asks generically.
    pub groups: crate::group_id::GroupStatuses,
}

fn quality_status(q: EstimateQuality) -> SignalStatus {
    match q {
        EstimateQuality::Good => SignalStatus::Valid,
        EstimateQuality::Degraded => SignalStatus::Degraded,
        EstimateQuality::Unusable | EstimateQuality::Unknown => SignalStatus::Failed,
    }
}

fn flag_status(valid: bool) -> SignalStatus {
    if valid {
        SignalStatus::Valid
    } else {
        SignalStatus::Failed
    }
}

pub(crate) fn fault_status<T>(fault: Option<T>) -> SignalStatus {
    if fault.is_some() {
        SignalStatus::Failed
    } else {
        SignalStatus::Valid
    }
}

/// Attitude and kinematics are stamped independently; when the ingress
/// gate reports their acquisition times exceed the skew budget, each
/// value is individually usable but the pair must not present as one
/// coherent aircraft state, so both groups degrade (amber, value shown).
/// An unknown coherence wire value degrades the same way — the pairing
/// cannot be trusted. `Insufficient` means too few stamped groups to
/// judge; the ordinary missing/freshness handling covers that case.
fn coherence_status(coherence: SnapshotCoherence) -> SignalStatus {
    match coherence {
        SnapshotCoherence::ExcessiveSkew | SnapshotCoherence::Unknown => SignalStatus::Degraded,
        SnapshotCoherence::Insufficient | SnapshotCoherence::Coherent => SignalStatus::Valid,
    }
}

/// A signal that would show a non-finite value fails instead: no
/// non-finite number may reach scene generation, and no value is
/// silently repaired.
pub(crate) fn finite(sig: Sig<f32>) -> Sig<f32> {
    if sig.status.shows_value() && !sig.value.is_finite() {
        Sig::with_status(0.0, SignalStatus::Failed)
    } else {
        sig
    }
}

fn sanitized_selections(selections: Selections) -> Selections {
    Selections {
        heading_bug_rad: if selections.heading_bug_rad.is_finite() {
            selections.heading_bug_rad
        } else {
            0.0
        },
        heading_bug_reference: selections.heading_bug_reference,
        altitude_sel_m: selections.altitude_sel_m.filter(|value| value.is_finite()),
        altitude_sel_class: selections.altitude_sel_class,
        altitude_sel_origin: selections.altitude_sel_origin,
        altitude_sel_model: selections.altitude_sel_model,
        baro_sel_hpa: selections.baro_sel_hpa.filter(|value| value.is_finite()),
    }
}

/// Resolves raw input state into display-ready signals.
///
/// Each signal's status is the deterministic worst of: its group's
/// freshness under `policy`, the source quality, the snapshot's
/// group-coherence result, the source's validity flag for that group,
/// and numeric/integrity validation ([`validate_state`]). Validity
/// flags apply only to groups with data — a group never received stays
/// `Missing`. Values behind `Missing`/`Failed` are quiet zeros a panel
/// never paints, and every showable value is finite.
pub fn resolve(state: &AircraftState, policy: &FreshnessPolicy) -> PanelData {
    let mut fresh = UnusualAttitudeState::default();
    resolve_stateful(
        state,
        policy,
        &AirframeDisplayProfile::simulator(),
        &mut fresh,
    )
}

/// [`resolve`] with a caller-held unusual-attitude latch state, so tier
/// entry/exit hysteresis works across frames. [`resolve`] itself uses a
/// fresh state per call (entry thresholds only), which is sufficient for
/// single-frame consumers and tests.
pub fn resolve_stateful(
    state: &AircraftState,
    policy: &FreshnessPolicy,
    profile: &AirframeDisplayProfile,
    unusual: &mut UnusualAttitudeState,
) -> PanelData {
    let integrity = validate_state(state);
    let trust = Trust {
        quality: quality_status(state.quality).worst(fault_status(integrity.quality)),
        coherence: coherence_status(state.snapshot.coherence),
    };

    let presentation = attitude_geometry(state, profile, unusual);
    let has_att = state.attitude.data.is_some();
    let att_fresh = group_freshness(policy, has_att, state.attitude.age_ms);
    let att_status = trust.fold(has_att, att_fresh, integrity.attitude, state.valid.attitude);

    let kin = kinematic_signals(state, policy, &trust, &integrity);

    let (ias, tas, baro) = air_signals(state, policy, trust.quality, &integrity);
    let heading = heading_resolved(state, policy, &trust, &integrity);
    let basis = rose_basis(&heading, kin.track_rad.status.shows_value());
    let rose = basis.display_reference(heading.reference);
    let track = presented_true(kin.track_rad, rose, state, policy);
    let wind = presented_wind(wind_signal(state, policy, &integrity), rose, state, policy);
    let groups = group_status::group_statuses(state, policy, &trust, &integrity);
    let bug = heading_bug_presented(state, policy, rose);

    let equipment = equipment_resolved(state, policy, &trust, &integrity, rose, &groups, &kin);
    PanelData {
        bearings_rose_rad: equipment.bearings_rose_rad,
        bearings: equipment.bearings,
        flight: flight_signal::flight_resolved(
            state, policy, &trust, &integrity, &groups, &equipment,
        ),
        roll_rad: finite(Sig::with_status(presentation.bank_rad, att_status)),
        pitch_rad: finite(Sig::with_status(presentation.pitch_rad, att_status)),
        heading,
        rose_basis: basis,
        heading_bug_rose_rad: finite(bug),
        turn: turn_resolved(state, policy, &trust, &integrity),
        slip_lat_mps2: slip_resolved(state, policy, &trust, &integrity),
        ias_trend_kt_s: ias_trend_resolved(state, policy, &trust, &integrity),
        ias_kt: finite(ias),
        tas_kt: finite(tas),
        gs_kt: finite(kin.gs_kt),
        altitude: equipment.altitude,
        vsi_fpm: finite(kin.vsi_fpm),
        track_rad: finite(track),
        baro_hpa: finite(baro),
        wind,
        nav: nav_resolved(state, policy, &integrity, rose, trust),
        selections: sanitized_selections(state.selections),
        integrity,
        presentation,
        require_dynamics_cue: profile.require_dynamics_cue,
        sources: crate::source_monitor::SourceSelection::default(),
        monitor_text: Sig::with_status(
            state.monitor_text.data.unwrap_or_default(),
            groups.status(crate::group_id::GroupId::MonitorText),
        ),
        airframe: Sig::with_status(
            state.airframe.data.unwrap_or_default(),
            groups.status(crate::group_id::GroupId::AirframeConfig),
        ),
        groups,
    }
}

impl Default for NavResolved {
    fn default() -> Self {
        Self {
            data: NavData::default(),
            status: SignalStatus::default(),
            course_rose_rad: Sig::with_status(0.0, SignalStatus::Missing),
        }
    }
}

/// Source-level trust shared by every estimate group this frame.
#[derive(Clone, Copy)]
pub(crate) struct Trust {
    pub(crate) quality: SignalStatus,
    pub(crate) coherence: SignalStatus,
}

impl Trust {
    /// The deterministic worst-of for one group. Trust metadata applies
    /// only to groups that have data: absence stays Missing — dashes,
    /// not a red X — because nothing was received to distrust. A group
    /// *with* data still folds even when its freshness reads Missing
    /// (a bogus age), so declared distrust cannot be masked.
    pub(crate) fn fold(
        &self,
        has_data: bool,
        freshness: SignalStatus,
        fault: Option<crate::validate::GroupFault>,
        declared_valid: bool,
    ) -> SignalStatus {
        if !has_data {
            return SignalStatus::Missing;
        }
        freshness
            .worst(self.quality)
            .worst(self.coherence)
            .worst(fault_status(fault))
            .worst(flag_status(declared_valid))
    }
}

pub(crate) fn group_freshness(
    policy: &FreshnessPolicy,
    has_data: bool,
    age_ms: Option<f32>,
) -> SignalStatus {
    if has_data {
        policy.status_for_age(age_ms)
    } else {
        SignalStatus::Missing
    }
}

/// Geometry only ever sees a validated, renormalized quaternion; a
/// rejected one resets the tier latches and leaves quiet zeros behind a
/// Failed status — never a plausible horizon.
fn attitude_geometry(
    state: &AircraftState,
    profile: &AirframeDisplayProfile,
    unusual: &mut UnusualAttitudeState,
) -> AttitudePresentation {
    match state.attitude.data {
        Some(att) => match validate_quat(att.quat) {
            Ok(quat) => unusual.step(quat, profile),
            Err(_) => {
                unusual.reset();
                AttitudePresentation::default()
            }
        },
        None => {
            unusual.reset();
            AttitudePresentation::default()
        }
    }
}

fn air_signals(
    state: &AircraftState,
    policy: &FreshnessPolicy,
    quality: SignalStatus,
    integrity: &StateIntegrity,
) -> (Sig<f32>, Sig<f32>, Sig<f32>) {
    let air_fresh = policy.status_for_age(state.air.age_ms);
    let air_fault = fault_status(integrity.air);
    let air = state.air.data.unwrap_or_default();
    // Sensed air-data values fold source quality; the applied altimeter
    // setting does not, because a setting is a dialed configuration
    // value and an estimate's quality says nothing about it.
    let ias = match air.ias_mps {
        Some(v) => Sig::with_status(v * MPS_TO_KT, air_fresh.worst(quality).worst(air_fault)),
        None => Sig::missing(),
    };
    let tas = match air.tas_mps {
        Some(v) => Sig::with_status(v * MPS_TO_KT, air_fresh.worst(quality).worst(air_fault)),
        None => Sig::missing(),
    };
    let baro = match air.baro_setting_hpa {
        Some(v) => Sig::with_status(v, air_fresh.worst(air_fault)),
        None => Sig::missing(),
    };
    (ias, tas, baro)
}

fn nav_resolved(
    state: &AircraftState,
    policy: &FreshnessPolicy,
    integrity: &StateIntegrity,
    rose: crate::heading::HeadingReference,
    trust: Trust,
) -> NavResolved {
    let nav_fresh = policy.status_for_age(state.nav.age_ms);
    match state.nav.data {
        Some(data) => {
            // Nav folds source trust like every other group: a source that
            // declares itself Unusable, or a snapshot whose groups cannot be
            // paired, must not draw a confident CDI. `ValidFlags` has no nav
            // bit, so the declared-valid input is neutral here; giving nav its
            // own bit is a wire and ABI change tracked separately.
            let status = trust.fold(true, nav_fresh, integrity.nav, true);
            // Guidance from an unidentifiable source must not draw a
            // CDI at all; failing the group removes it.
            let data = if matches!(data.source, NavSource::Unknown) {
                NavData {
                    source: NavSource::Unknown,
                    ..NavData::default()
                }
            } else {
                data
            };
            // The course renders only in the rose reference; an
            // unknown course reference or an impossible conversion
            // fails this one quantity, not the whole nav group.
            let course = presented_angle(
                Sig::with_status(data.course_rad, status),
                data.course_reference,
                rose,
                state,
                policy,
            );
            NavResolved {
                data,
                status,
                course_rose_rad: finite(course),
            }
        }
        None => NavResolved::default(),
    }
}

mod altitude_signal;
mod autoflight_signal;
mod bearings_signal;
mod dynamics_signal;
mod equipment_signal;
mod flight_signal;
mod group_status;
mod kinematics_signal;
mod wind_signal;
use dynamics_signal::{ias_trend_resolved, slip_resolved, turn_resolved};
use equipment_signal::equipment_resolved;
use kinematics_signal::kinematic_signals;
use wind_signal::wind_signal;
mod heading_signal;
pub use heading_signal::{ResolvedHeading, RoseBasis};
mod director_signal;
pub use director_signal::ResolvedDirector;
use heading_signal::heading_bug_presented;
use heading_signal::rose_basis;
use heading_signal::{heading_resolved, presented_angle, presented_true, presented_wind};

#[cfg(test)]
mod altitude_tests;
#[cfg(test)]
mod dynamics_tests;
#[cfg(test)]
mod heading_tests;
#[cfg(test)]
mod tests;
