//! The raw input state a feeder writes.

use crate::altitude::{AltitudeClass, AltitudeDeclaration, GeoidModelId, OriginId};
use crate::dynamics::DynSample;
use crate::heading::{HeadingReference, HeadingSample, MagneticVariation};
use crate::ident::IdentStr;
use indicate_frames::Quat;

/// Attitude estimate: orientation and body rotation rates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Attitude {
    /// Body→NED rotation.
    pub quat: Quat,
    /// Body rates (p, q, r) in radians/second.
    pub rates_rps: [f32; 3],
}

/// Kinematic estimate in the local NED frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Kinematics {
    /// Position (north, east, down) in meters from the local origin.
    pub pos_ned_m: [f32; 3],
    /// Velocity (north, east, down) in meters/second.
    pub vel_ned_mps: [f32; 3],
}

/// Air data. Every field is optional because vehicles without the sensor
/// must display `Missing`, not a substitute (ADR-0017).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AirData {
    /// Indicated airspeed in meters/second.
    pub ias_mps: Option<f32>,
    /// Altimeter setting in hectopascals.
    pub baro_setting_hpa: Option<f32>,
    /// True airspeed in meters/second. The display cannot derive it —
    /// that needs density the state does not carry — so a source that
    /// does not supply it leaves it absent and the readout shows
    /// `Missing`, never a number computed from indicated airspeed.
    pub tas_mps: Option<f32>,
}

/// The selected lateral navigation source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NavSource {
    /// No source selected; the HSI is a directional gyro.
    #[default]
    None,
    /// GPS/FMS course (magenta).
    Gps,
    /// NAV radio 1 (green).
    Nav1,
    /// NAV radio 2 (green).
    Nav2,
    /// The wire carried a source this build does not know. Guidance from
    /// an unidentifiable source must not display; the nav group fails
    /// rather than quietly pretending no source is selected.
    Unknown,
}

/// TO/FROM resolution of the selected course.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NavFromTo {
    /// No valid course guidance; the deviation bar is removed.
    #[default]
    Off,
    /// Flying toward the station/waypoint.
    To,
    /// Flying away from the station/waypoint.
    From,
    /// The wire carried a resolution this build does not know; the nav
    /// group fails rather than defaulting to a benign flag state.
    Unknown,
}

/// Lateral/vertical course guidance from the selected source.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct NavData {
    /// Which source drives the deviation bar.
    pub source: NavSource,
    /// Selected course in radians from ITS OWN declared north.
    pub course_rad: f32,
    /// The north the course is expressed against. The CDI and course
    /// box render only after conversion into the rose reference.
    pub course_reference: HeadingReference,
    /// Lateral deviation in dots (full scale ±2).
    pub cdi_dots: f32,
    /// TO/FROM flag.
    pub fromto: NavFromTo,
    /// Vertical deviation in dots (full scale ±2.5), when available.
    pub vdev_dots: Option<f32>,
    /// Distance to the waypoint/station in nautical miles.
    pub dist_nm: Option<f32>,
    /// Active (TO) waypoint ident; empty renders dashes, and malformed
    /// wire content decodes [`IdentStr::INVALID`], failing the group.
    pub to_ident: IdentStr,
    /// Previous (FROM) waypoint ident; same rules as `to_ident`.
    pub from_ident: IdentStr,
    /// What full-scale deflection means for this guidance.
    pub scale: NavScale,
}

/// The deflection scale the guidance source is flying to.
///
/// Two dots is two dots on the glass whatever the phase, so the same
/// needle position means a different distance in each. A source that
/// changes scale without saying so changes the meaning of the picture
/// and nothing on the panel could tell. The scale is therefore
/// declared, never inferred from a distance the display was handed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NavScale {
    /// Enroute.
    Enroute,
    /// Terminal.
    Terminal,
    /// Approach.
    Approach,
    /// The wire carried a scale this build does not know, or none was
    /// declared. Guidance whose scale is unknown means nothing, so the
    /// nav group fails rather than drawing at a guessed scale.
    #[default]
    Unknown,
}

impl NavScale {
    /// Fail-closed wire decoding.
    #[must_use]
    pub const fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Enroute,
            1 => Self::Terminal,
            2 => Self::Approach,
            _ => Self::Unknown,
        }
    }

    /// Wire encoding; `Unknown` round-trips as unknown.
    #[must_use]
    pub const fn to_u8(self) -> u8 {
        match self {
            Self::Enroute => 0,
            Self::Terminal => 1,
            Self::Approach => 2,
            Self::Unknown => 255,
        }
    }

    /// The label that names this scale to the pilot. Every character is
    /// in the panel glyph vocabulary, which has no `+` and no `/`, so
    /// the richer per-approach names cannot be spelled here yet.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Enroute => "ENR",
            Self::Terminal => "TERM",
            Self::Approach => "APR",
            Self::Unknown => "",
        }
    }
}

/// Pilot selections and bugs. These are local UI state, not sensed data,
/// so they carry no freshness.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Selections {
    /// Heading bug in radians from ITS OWN declared north.
    pub heading_bug_rad: f32,
    /// The north the heading bug is expressed against. The bug renders
    /// only after conversion into the rose reference; unknown fails.
    pub heading_bug_reference: HeadingReference,
    /// Selected altitude in meters, when set.
    pub altitude_sel_m: Option<f32>,
    /// Reference class the selected altitude is expressed in. The bug
    /// and selection readout render only against a compatible displayed
    /// reference — numeric equality across references means nothing,
    /// and class equality alone is not identity: the class-specific
    /// identity below must match too.
    pub altitude_sel_class: AltitudeClass,
    /// Origin identity of a local-relative selection. A selection made
    /// against origin A is not a selection against origin B.
    pub altitude_sel_origin: OriginId,
    /// Geoid-model identity of a geometric-MSL selection; undeclared is
    /// an incomplete identity and never compatible.
    pub altitude_sel_model: GeoidModelId,
    /// Pilot-selected altimeter setting in hectopascals. Selection is
    /// UI state; the sensed/applied setting lives in [`AirData`], and a
    /// disagreement between the two is flagged, never averaged.
    pub baro_sel_hpa: Option<f32>,
}

/// One bearing pointer: which receiver it follows and where that
/// receiver says the station is.
///
/// A pointer is independent of the CDI: it can follow a receiver the
/// course selector is not on, which is the whole reason to have one.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BearingPointer {
    /// Which receiver drives this pointer.
    pub source: NavSource,
    /// Bearing to the station in radians from ITS OWN declared north.
    pub bearing_rad: f32,
    /// The north the bearing is expressed against. A pointer renders
    /// only after conversion into the rose reference; unknown fails.
    pub reference: HeadingReference,
    /// The source declares this bearing usable. A pointer whose source
    /// says otherwise is removed, never parked.
    pub valid: bool,
}

/// The bearing pointers, in draw order.
///
/// Two, because the panel draws two distinct needle forms and a pilot
/// tells them apart by shape. A pointer whose source is `None` is not
/// drawn; one whose source this build cannot name fails the group,
/// because a needle pointing somewhere on behalf of nobody is worse
/// than no needle.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BearingPointers {
    /// The single-line needle.
    pub first: BearingPointer,
    /// The double-line needle.
    pub second: BearingPointer,
}

/// Airframe configuration: what the airframe is set to, as distinct
/// from what it is doing.
///
/// Every field is optional because a vehicle without the sensor must
/// display `Missing`, not a substitute (ADR-0017). Sensed and selected
/// are never conflated: a detent the pilot chose is not a position the
/// airframe reached, and a disagreement between them is a fact worth
/// showing rather than averaging away.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AirframeConfig {
    /// Sensed flap position, 0.0 retracted to 1.0 fully extended.
    pub flap_ratio: Option<f32>,
    /// The detent the pilot selected, in the same units. Absent when the
    /// airframe has no detented selector or the source does not report
    /// one.
    pub flap_selected_ratio: Option<f32>,
    /// Elevator trim, -1.0 fully nose-down to 1.0 fully nose-up.
    pub elevator_trim_ratio: Option<f32>,
    /// Aileron trim, -1.0 fully left-wing-down to 1.0 right.
    pub aileron_trim_ratio: Option<f32>,
    /// Rudder trim, -1.0 fully nose-left to 1.0 nose-right.
    pub rudder_trim_ratio: Option<f32>,
}

/// Wind estimate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wind {
    /// Direction the wind blows *from*, radians clockwise from north.
    pub from_rad: f32,
    /// Speed in meters/second.
    pub speed_mps: f32,
}

/// Source-reported estimate quality (mirrors Aviate's `EstimateQuality`).
///
/// Trust must be declared, never assumed: the default is [`Self::Unknown`],
/// and a wire value outside the known set decodes to `Unknown` rather than
/// to a benign level. Unknown quality resolves `Failed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EstimateQuality {
    /// Full confidence.
    Good,
    /// Reduced confidence; signals show `Degraded`.
    Degraded,
    /// The source says do not trust; signals show `Failed`.
    Unusable,
    /// No quality was declared, or the declared value is not one this
    /// build knows; signals show `Failed`.
    #[default]
    Unknown,
}

/// Which estimate groups the source declares valid (mirrors Aviate's
/// `StateValidFlags`).
///
/// The default declares nothing valid: a feeder that never sets the flags
/// gets `Failed` groups, not silently trusted ones. Flags apply only to
/// groups that have data — a group never received stays `Missing`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ValidFlags {
    /// Attitude quaternion is valid.
    pub attitude: bool,
    /// Body rates are valid.
    pub rates: bool,
    /// NED position is valid.
    pub position: bool,
    /// The north/east velocity components are valid — the pair ground
    /// speed and track are read from. A source with a horizontal
    /// solution and no vertical-speed estimate declares this alone.
    pub velocity_horizontal: bool,
    /// The down velocity component is valid — the one vertical speed is
    /// read from, declared independently of the horizontal pair so
    /// neither can borrow the other's trust.
    pub velocity_vertical: bool,
    /// The heading sample is declared valid.
    pub heading: bool,
    /// The variation sample is declared valid.
    pub variation: bool,
    /// The turn-rate sample is declared valid.
    pub turn: bool,
    /// The lateral-force (slip/skid) sample is declared valid.
    pub slip: bool,
    /// The airspeed-trend sample is declared valid.
    pub ias_trend: bool,
}

/// One estimate group with the age a feeder stamped it with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stamped<T> {
    /// The data, absent until first received.
    pub data: Option<T>,
    /// Milliseconds since last update; `None` when never received.
    pub age_ms: Option<f32>,
}

impl<T> Default for Stamped<T> {
    fn default() -> Self {
        Self {
            data: None,
            age_ms: None,
        }
    }
}

/// Whether independently acquired groups form one coherent display snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SnapshotCoherence {
    /// Too few stamped groups are present to establish coherence.
    #[default]
    Insufficient,
    /// Required groups share a source epoch/clock and meet the skew budget.
    Coherent,
    /// Required groups exceed the configured acquisition-time skew budget.
    ExcessiveSkew,
    /// The wire carried a coherence value this build does not know; the
    /// pairing cannot be trusted, so stamped groups degrade.
    Unknown,
}

/// Metadata the input validation and time/integrity gate assigns to one
/// immutable state snapshot.
///
/// The gate assigns it, never a source: a source is on the untrusted
/// side of the boundary this metadata exists to describe, so a counter
/// a source supplied would be the thing being judged rather than the
/// judgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SnapshotMeta {
    /// Wrapping generation: one step per snapshot the gate admits,
    /// whatever the groups hold. It counts admission, not content, so
    /// a consumer detecting a content change compares content. A gate
    /// that admits nothing new still advances it; a gate that stopped
    /// admitting does not, which is what a liveness check reads.
    pub generation: u32,
    /// Coherence result for the independently stamped input groups.
    pub coherence: SnapshotCoherence,
}

/// The unified input state every instrument reads (ADR-0017).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AircraftState {
    /// Optional shared guidance and speed-presentation reports.
    pub flight: crate::flight::FlightInputs,
    /// Attitude group.
    pub attitude: Stamped<Attitude>,
    /// Kinematics group.
    pub kinematics: Stamped<Kinematics>,
    /// Air-data group.
    pub air: Stamped<AirData>,
    /// Navigation guidance group.
    pub nav: Stamped<NavData>,
    /// Wind estimate group.
    pub wind: Stamped<Wind>,
    /// Pilot selections.
    pub selections: Selections,
    /// Source quality.
    pub quality: EstimateQuality,
    /// Source validity flags.
    pub valid: ValidFlags,
    /// The gate's generation and group-coherence result.
    pub snapshot: SnapshotMeta,
    /// Datum declaration for the primary altitude (ALT-01).
    pub altitude: AltitudeDeclaration,
    /// Independent heading sample with an explicit reference (NAV-01).
    /// Operational heading never derives implicitly from attitude yaw.
    pub heading: Stamped<HeadingSample>,
    /// Magnetic-variation sample for the single sanctioned
    /// magnetic/true conversion path.
    pub variation: Stamped<MagneticVariation>,
    /// Typed turn and slip/skid estimates (DYN-01); body rates never
    /// substitute for either.
    pub dynamics: Stamped<DynSample>,
    /// Flight-director commanded attitude, mode, and engagement.
    pub director: Stamped<crate::director::FdSample>,
    /// Machine-monitoring text channel (AIR-IN-014); advisory content
    /// with its own slow freshness policy, never flight data.
    pub monitor_text: Stamped<crate::monitor_text::MonitorText>,
    /// Bearing pointers, independent of the selected nav source.
    pub bearings: Stamped<BearingPointers>,
    /// Airframe configuration: flap position and trim.
    pub airframe: Stamped<AirframeConfig>,
    /// Autoflight engagement and the active and armed modes
    /// (AIR-IN-015). Stamped: an annunciation that outlives its source
    /// says the automation is doing something it stopped doing.
    pub ap_modes: Stamped<crate::autopilot::ApModes>,
    /// The values the automation is flying toward (AIR-IN-015).
    /// Declared beside the other selections: a target is UI state in
    /// the same sense a heading bug is.
    pub ap_targets: crate::autopilot::ApTargets,
}

impl Default for Selections {
    fn default() -> Self {
        Self {
            heading_bug_rad: 0.0,
            heading_bug_reference: HeadingReference::Unknown,
            altitude_sel_m: None,
            altitude_sel_class: AltitudeClass::LocalRelative,
            altitude_sel_origin: OriginId(0),
            altitude_sel_model: GeoidModelId::UNDECLARED,
            baro_sel_hpa: None,
        }
    }
}
