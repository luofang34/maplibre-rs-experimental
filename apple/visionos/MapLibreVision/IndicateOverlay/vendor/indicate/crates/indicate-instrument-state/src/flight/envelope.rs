//! Conditional intervals retain their causes, coverage, and gaps.

/// Maximum intervals in a bounded display report.
pub const MAX_ENVELOPE_INTERVALS: usize = 8;

/// Completeness of the producer's constraint evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum EnvelopeCoverage {
    /// All constraints required by the declared model were evaluated.
    Complete = 0,
    /// Some required constraints could not be evaluated.
    Partial = 1,
    /// No usable envelope is available.
    Unavailable = 2,
    /// Coverage was not understood.
    #[default]
    Unknown = 255,
}

impl EnvelopeCoverage {
    /// Decodes coverage without treating missing constraints as complete.
    pub fn from_u8(value: u8) -> Self {
        [Self::Complete, Self::Partial, Self::Unavailable]
            .get(value as usize)
            .copied()
            .unwrap_or(Self::Unknown)
    }
}

/// Meaning of an interval; different meanings are never merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum EnvelopeKind {
    /// Region within published operating limits.
    Operating = 0,
    /// Region bounded by current protection thresholds.
    Protection = 1,
    /// Region with the declared maneuver or environmental margin.
    Awareness = 2,
    /// Region satisfying a declared performance condition.
    Performance = 3,
    /// Region preferred by the mission or guidance system.
    Preferred = 4,
    /// Interval meaning was not understood.
    #[default]
    Unknown = 255,
}

impl EnvelopeKind {
    /// Decodes the interval meaning.
    pub fn from_u8(value: u8) -> Self {
        [
            Self::Operating,
            Self::Protection,
            Self::Awareness,
            Self::Performance,
            Self::Preferred,
        ]
        .get(value as usize)
        .copied()
        .unwrap_or(Self::Unknown)
    }
}

/// Physical or operational cause of a boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum BoundaryReason {
    /// Stall warning threshold.
    StallWarning = 0,
    /// Angle-of-attack threshold.
    AngleOfAttack = 1,
    /// Flap extension limit.
    Flaps = 2,
    /// Landing gear limit.
    Gear = 3,
    /// Maximum operating airspeed.
    Vmo = 4,
    /// Maximum operating Mach number.
    Mmo = 5,
    /// Dynamic-pressure limit.
    DynamicPressure = 6,
    /// Buffet threshold.
    Buffet = 7,
    /// Control authority limit.
    Control = 8,
    /// Thermal limit under the declared condition.
    Thermal = 9,
    /// Propulsion capability limit.
    Propulsion = 10,
    /// Structural limit.
    Structural = 11,
    /// Maneuver margin.
    Maneuver = 12,
    /// Mission preference.
    Mission = 13,
    /// Boundary cause was not understood.
    #[default]
    Unknown = 255,
}

impl BoundaryReason {
    /// Decodes a cause without inventing a physical meaning.
    pub fn from_u8(value: u8) -> Self {
        [
            Self::StallWarning,
            Self::AngleOfAttack,
            Self::Flaps,
            Self::Gear,
            Self::Vmo,
            Self::Mmo,
            Self::DynamicPressure,
            Self::Buffet,
            Self::Control,
            Self::Thermal,
            Self::Propulsion,
            Self::Structural,
            Self::Maneuver,
            Self::Mission,
        ]
        .get(value as usize)
        .copied()
        .unwrap_or(Self::Unknown)
    }

    /// Glyph-covered label for the limiting cause.
    pub const fn label(self) -> &'static str {
        match self {
            Self::StallWarning => "STALL WARN",
            Self::AngleOfAttack => "AOA",
            Self::Flaps => "VFE",
            Self::Gear => "GEAR",
            Self::Vmo => "VMO",
            Self::Mmo => "MMO",
            Self::DynamicPressure => "Q",
            Self::Buffet => "BUFFET",
            Self::Control => "CTRL",
            Self::Thermal => "THERM",
            Self::Propulsion => "THR",
            Self::Structural => "STRUCT",
            Self::Maneuver => "MANEUVER",
            Self::Mission => "PREF",
            Self::Unknown => "LIMIT",
        }
    }
}

/// One connected interval in the declared display coordinate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnvelopeInterval {
    /// Meaning of the interval.
    pub kind: EnvelopeKind,
    /// Lower boundary in coordinate SI units.
    pub lower: f32,
    /// Upper boundary in coordinate SI units.
    pub upper: f32,
    /// Cause of the lower boundary.
    pub lower_reason: BoundaryReason,
    /// Cause of the upper boundary.
    pub upper_reason: BoundaryReason,
}

/// Model-supplied intervals for one explicitly identified aircraft condition.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ProjectedEnvelope {
    /// Completeness of the declared constraint set.
    pub coverage: EnvelopeCoverage,
    /// Nonzero aircraft model revision identifier.
    pub model_id: u32,
    /// Nonzero identifier for the evaluated state, configuration, and time assumptions.
    pub condition_id: u32,
    /// Ordered, nonoverlapping intervals within each kind; empty slots carry no interval.
    pub intervals: [Option<EnvelopeInterval>; MAX_ENVELOPE_INTERVALS],
}
