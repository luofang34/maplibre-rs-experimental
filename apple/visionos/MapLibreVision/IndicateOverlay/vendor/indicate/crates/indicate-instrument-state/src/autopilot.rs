//! Autoflight engagement, modes, and targets.
//!
//! A pilot flying against automation has two questions the rest of the
//! display cannot answer: what is the automation doing now, and what is
//! it about to do. The active mode answers the first and the armed mode
//! answers the second, so they are separate fields rather than one
//! field the display guesses a tense for.
//!
//! Every mode enum carries `None` and `Unknown`, and they are not the
//! same thing. `None` is the automation saying it holds nothing on that
//! axis. `Unknown` is a wire value this build cannot read, which fails
//! the group: a mode nobody can name must not annunciate as a mode
//! somebody could act on.
//!
//! Modes and engagement sit on the stamped lane. A frozen "AP ENGAGED"
//! is the failure the freshness discipline exists to prevent — the
//! annunciation must go stale with its source, not outlive it. The
//! target values sit on the declared lane beside the other selections,
//! because a selected airspeed is UI state in the same sense a heading
//! bug is.
//!
//! No shipped posture publishes either group: until a feeder wires an
//! upstream autopilot, both resolve `Missing` everywhere and the
//! annunciator draws nothing. The contract is live, the data is not.

use crate::altitude::{AltitudeClass, GeoidModelId, OriginId};

/// What is flying the aircraft.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ApEngagement {
    /// Nothing: no guidance and no servo.
    Off,
    /// A flight director guides; the pilot flies.
    FlightDirector,
    /// The autopilot flies.
    Autopilot,
    /// The wire carried an engagement this build does not know; the
    /// group fails rather than guessing which of the three it meant.
    #[default]
    Unknown,
}

impl ApEngagement {
    /// Fail-closed wire decoding.
    #[must_use]
    pub const fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Off,
            1 => Self::FlightDirector,
            2 => Self::Autopilot,
            _ => Self::Unknown,
        }
    }

    /// Wire encoding; `Unknown` round-trips as unknown.
    #[must_use]
    pub const fn to_u8(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::FlightDirector => 1,
            Self::Autopilot => 2,
            Self::Unknown => 255,
        }
    }

    /// The annunciation label. `Off` and `Unknown` have none: an
    /// engagement nobody declared is not annunciated as one.
    #[must_use]
    pub const fn label(self) -> Option<&'static str> {
        match self {
            Self::FlightDirector => Some("FD"),
            Self::Autopilot => Some("AP"),
            Self::Off | Self::Unknown => None,
        }
    }
}

/// What the automation holds or tracks in the lateral axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LateralMode {
    /// The automation declares no lateral mode.
    None,
    /// Wings-level or a held bank.
    Roll,
    /// Track the selected heading.
    Heading,
    /// Track lateral navigation guidance.
    Nav,
    /// Track an approach's lateral guidance.
    Approach,
    /// A wire value this build does not know; the group fails.
    #[default]
    Unknown,
}

impl LateralMode {
    /// Fail-closed wire decoding.
    #[must_use]
    pub const fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::None,
            1 => Self::Roll,
            2 => Self::Heading,
            3 => Self::Nav,
            4 => Self::Approach,
            _ => Self::Unknown,
        }
    }

    /// Wire encoding; `Unknown` round-trips as unknown.
    #[must_use]
    pub const fn to_u8(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Roll => 1,
            Self::Heading => 2,
            Self::Nav => 3,
            Self::Approach => 4,
            Self::Unknown => 255,
        }
    }

    /// The annunciation label, or none when there is no mode to name.
    #[must_use]
    pub const fn label(self) -> Option<&'static str> {
        match self {
            Self::Roll => Some("ROL"),
            Self::Heading => Some("HDG"),
            Self::Nav => Some("NAV"),
            Self::Approach => Some("APR"),
            Self::None | Self::Unknown => None,
        }
    }
}

/// What the automation holds or tracks in the vertical axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VerticalMode {
    /// The automation declares no vertical mode.
    None,
    /// Hold the current pitch.
    Pitch,
    /// Hold the current altitude.
    Altitude,
    /// Level off at the altitude target.
    AltitudeCapture,
    /// Hold the target vertical speed.
    VerticalSpeed,
    /// Hold the target airspeed with pitch.
    Airspeed,
    /// Track an approach's vertical guidance.
    GlideSlope,
    /// A wire value this build does not know; the group fails.
    #[default]
    Unknown,
}

impl VerticalMode {
    /// Fail-closed wire decoding.
    #[must_use]
    pub const fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::None,
            1 => Self::Pitch,
            2 => Self::Altitude,
            3 => Self::AltitudeCapture,
            4 => Self::VerticalSpeed,
            5 => Self::Airspeed,
            6 => Self::GlideSlope,
            _ => Self::Unknown,
        }
    }

    /// Wire encoding; `Unknown` round-trips as unknown.
    #[must_use]
    pub const fn to_u8(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Pitch => 1,
            Self::Altitude => 2,
            Self::AltitudeCapture => 3,
            Self::VerticalSpeed => 4,
            Self::Airspeed => 5,
            Self::GlideSlope => 6,
            Self::Unknown => 255,
        }
    }

    /// The annunciation label, or none when there is no mode to name.
    ///
    /// Altitude capture and altitude hold read differently on purpose:
    /// one is a level-off in progress and the other is a level-off that
    /// finished, and a pilot watching for the transition needs to see
    /// it happen.
    #[must_use]
    pub const fn label(self) -> Option<&'static str> {
        match self {
            Self::Pitch => Some("PIT"),
            Self::Altitude => Some("ALT"),
            Self::AltitudeCapture => Some("ALTS"),
            Self::VerticalSpeed => Some("VS"),
            Self::Airspeed => Some("IAS"),
            Self::GlideSlope => Some("GS"),
            Self::None | Self::Unknown => None,
        }
    }
}

/// One autoflight mode sample: what is engaged, and what each axis
/// holds now and is armed to hold next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ApModes {
    /// What is flying the aircraft.
    pub engagement: ApEngagement,
    /// The lateral mode in control now.
    pub lateral_active: LateralMode,
    /// The lateral mode that will take control at its capture point.
    pub lateral_armed: LateralMode,
    /// The vertical mode in control now.
    pub vertical_active: VerticalMode,
    /// The vertical mode that will take control at its capture point.
    pub vertical_armed: VerticalMode,
}

/// The values the automation is flying toward.
///
/// Each is optional because an automation holding altitude has no
/// airspeed target to report, and an absent target is not a zero one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ApTargets {
    /// Target indicated airspeed in meters per second, when set.
    pub airspeed_mps: Option<f32>,
    /// Target vertical speed in meters per second, positive up, when
    /// set.
    pub vertical_speed_mps: Option<f32>,
    /// Target altitude in meters, when set.
    pub altitude_m: Option<f32>,
    /// Reference class the target altitude is expressed in. The readout
    /// draws only against a compatible displayed reference: numeric
    /// equality across references means nothing.
    pub altitude_class: AltitudeClass,
    /// Origin identity of a local-relative target. A target against
    /// origin A is not a target against origin B.
    pub altitude_origin: OriginId,
    /// Geoid-model identity of a geometric-MSL target; undeclared is an
    /// incomplete identity and never compatible.
    pub altitude_model: GeoidModelId,
}

impl Default for ApTargets {
    /// The fail-closed default, matching [`crate::aircraft::Selections`]
    /// field for field: no target, and an altitude identity that is
    /// incomplete rather than plausible.
    fn default() -> Self {
        Self {
            airspeed_mps: None,
            vertical_speed_mps: None,
            altitude_m: None,
            altitude_class: AltitudeClass::LocalRelative,
            altitude_origin: OriginId(0),
            altitude_model: GeoidModelId::UNDECLARED,
        }
    }
}
