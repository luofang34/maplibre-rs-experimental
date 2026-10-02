//! Engagement, mode, and command reports from the flight guidance system.

use super::{SolutionId, speed::SpeedValue};
use crate::{FdEngagement, LateralMode, VerticalMode};

/// Active and armed modes for one control channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ModePair<T> {
    /// Mode operating now.
    pub active: T,
    /// Mode waiting for its capture condition.
    pub armed: T,
}

/// Automatic thrust channel mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum ThrustMode {
    /// No thrust mode is selected.
    None = 0,
    /// Thrust controls speed.
    Speed = 1,
    /// Thrust holds a commanded thrust value.
    Thrust = 2,
    /// Idle thrust is commanded.
    Idle = 3,
    /// Takeoff thrust is commanded.
    Takeoff = 4,
    /// Go-around thrust is commanded.
    GoAround = 5,
    /// The mode cannot be interpreted.
    #[default]
    Unknown = 255,
}

impl ThrustMode {
    /// Decodes a mode without guessing unknown values.
    pub fn from_u8(value: u8) -> Self {
        [
            Self::None,
            Self::Speed,
            Self::Thrust,
            Self::Idle,
            Self::Takeoff,
            Self::GoAround,
        ]
        .get(value as usize)
        .copied()
        .unwrap_or(Self::Unknown)
    }

    /// Stable flight mode annunciation.
    pub const fn label(self) -> Option<&'static str> {
        match self {
            Self::None | Self::Unknown => None,
            Self::Speed => Some("SPD"),
            Self::Thrust => Some("THR"),
            Self::Idle => Some("IDLE"),
            Self::Takeoff => Some("TO"),
            Self::GoAround => Some("GA"),
        }
    }
}

/// Channel currently responsible for speed control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum SpeedControl {
    /// No channel controls speed.
    None = 0,
    /// Pitch controls speed.
    Pitch = 1,
    /// Thrust controls speed.
    Thrust = 2,
    /// Speed-control authority is unknown.
    #[default]
    Unknown = 255,
}

impl SpeedControl {
    /// Decodes the declared control channel.
    pub fn from_u8(value: u8) -> Self {
        [Self::None, Self::Pitch, Self::Thrust]
            .get(value as usize)
            .copied()
            .unwrap_or(Self::Unknown)
    }
}

/// Current speed-protection state, independent of the target selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum ProtectionState {
    /// Protection is available and inactive.
    Inactive = 0,
    /// Low-speed protection is active.
    LowSpeed = 1,
    /// High-speed protection is active.
    HighSpeed = 2,
    /// Speed protection is unavailable.
    Unavailable = 3,
    /// Protection state is unknown.
    #[default]
    Unknown = 255,
}

impl ProtectionState {
    /// Decodes the declared protection state.
    pub fn from_u8(value: u8) -> Self {
        [
            Self::Inactive,
            Self::LowSpeed,
            Self::HighSpeed,
            Self::Unavailable,
        ]
        .get(value as usize)
        .copied()
        .unwrap_or(Self::Unknown)
    }
}

/// Reason for a producer-reported guidance transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum TransitionReason {
    /// No transition is being reported.
    None = 0,
    /// A selected mode became active.
    Selection = 1,
    /// An armed mode captured.
    Capture = 2,
    /// Guidance reverted to another mode.
    Reversion = 3,
    /// Speed control changed its reference coordinate.
    Crossover = 4,
    /// Protection changed the effective command.
    Protection = 5,
    /// The transition cannot be interpreted.
    #[default]
    Unknown = 255,
}

impl TransitionReason {
    /// Decodes the producer's transition reason.
    pub fn from_u8(value: u8) -> Self {
        [
            Self::None,
            Self::Selection,
            Self::Capture,
            Self::Reversion,
            Self::Crossover,
            Self::Protection,
        ]
        .get(value as usize)
        .copied()
        .unwrap_or(Self::Unknown)
    }
}

/// Aircraft attitude commanded by the flight director.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AttitudeCommand {
    /// Pitch in radians, positive nose-up.
    pub pitch_rad: f32,
    /// Bank in radians, positive right-wing-down.
    pub roll_rad: f32,
}

/// Producer-reported transition timing.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GuidanceTransition {
    /// Operational reason for the transition.
    pub reason: TransitionReason,
    /// Elapsed milliseconds at sample acquisition; absent when no transition exists.
    pub elapsed_ms: Option<f32>,
}

/// One coherent flight guidance report.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GuidanceSample {
    /// Identity used to join effective target projections.
    pub identity: SolutionId,
    /// Autopilot servo engagement; independent of the flight director.
    pub autopilot: FdEngagement,
    /// Flight director engagement.
    pub director: FdEngagement,
    /// Automatic thrust engagement.
    pub autothrust: FdEngagement,
    /// Lateral active and armed modes.
    pub lateral: ModePair<LateralMode>,
    /// Vertical active and armed modes.
    pub vertical: ModePair<VerticalMode>,
    /// Thrust active and armed modes.
    pub thrust: ModePair<ThrustMode>,
    /// Current speed-control channel.
    pub speed_control: SpeedControl,
    /// Effective native speed target; never inferred from a selection.
    pub speed_target: Option<SpeedValue>,
    /// Optional attitude command from this guidance computation.
    pub command: Option<AttitudeCommand>,
    /// Current speed-protection state.
    pub protection: ProtectionState,
    /// Operational transition with producer-owned timing.
    pub transition: GuidanceTransition,
}
