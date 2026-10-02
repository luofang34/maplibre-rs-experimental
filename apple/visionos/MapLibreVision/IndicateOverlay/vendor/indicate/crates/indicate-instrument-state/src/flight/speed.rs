//! Native speed quantities and source-supplied display projections.

use super::{SolutionId, envelope::ProjectedEnvelope};

/// Identity and SI unit of a speed-display coordinate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum SpeedCoordinate {
    /// Indicated airspeed, metres per second.
    Ias = 0,
    /// Calibrated airspeed, metres per second.
    Cas = 1,
    /// Equivalent airspeed, metres per second.
    Eas = 2,
    /// Mach number, dimensionless.
    Mach = 3,
    /// Dynamic pressure, pascals.
    DynamicPressure = 4,
    /// Coordinate identity was not understood.
    #[default]
    Unknown = 255,
}

impl SpeedCoordinate {
    /// Decodes a coordinate without substituting IAS.
    pub fn from_u8(value: u8) -> Self {
        [
            Self::Ias,
            Self::Cas,
            Self::Eas,
            Self::Mach,
            Self::DynamicPressure,
        ]
        .get(value as usize)
        .copied()
        .unwrap_or(Self::Unknown)
    }

    /// Display label including the unit where needed.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ias => "IAS KT",
            Self::Cas => "CAS KT",
            Self::Eas => "EAS KT",
            Self::Mach => "MACH",
            Self::DynamicPressure => "Q PA",
            Self::Unknown => "SPD REF",
        }
    }

    /// Converts SI magnitudes into this coordinate's display units.
    pub fn display_value(self, value: f32) -> f32 {
        match self {
            Self::Ias | Self::Cas | Self::Eas => value * crate::units::MPS_TO_KT,
            _ => value,
        }
    }
}

/// A magnitude with an explicit physical coordinate.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SpeedValue {
    /// Physical quantity represented by the value.
    pub coordinate: SpeedCoordinate,
    /// Native magnitude in the coordinate's SI unit.
    pub value: f32,
}

impl SpeedValue {
    /// Whether the coordinate and magnitude can be displayed.
    pub fn is_valid(self) -> bool {
        self.coordinate != SpeedCoordinate::Unknown
            && self.value.is_finite()
            && self.value >= 0.0
            && self.coordinate.display_value(self.value).is_finite()
    }
}

/// A native target and its optional projection into the display coordinate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProjectedTarget {
    /// Selection or effective target in its control reference.
    pub native: SpeedValue,
    /// Projected magnitude in the speed sample's coordinate; absent when conversion is unavailable.
    pub projected: Option<f32>,
}

/// One speed-presentation solution computed under a single aircraft condition.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SpeedSample {
    /// Guidance computation associated with the effective target projection.
    pub guidance_identity: SolutionId,
    /// Explicit display coordinate; independent of the control reference.
    pub coordinate: SpeedCoordinate,
    /// Current measured magnitude in the coordinate's SI unit.
    pub current: Option<f32>,
    /// Source-supplied magnitude rate in coordinate units per second.
    pub rate: Option<f32>,
    /// Contextual speed value, such as Mach beside an IAS tape.
    pub secondary: Option<SpeedValue>,
    /// Selected target, without an assertion that a controller follows it.
    pub selected: Option<ProjectedTarget>,
    /// Effective target projection; requires matching guidance identity and native target.
    pub effective: Option<ProjectedTarget>,
    /// Model-supplied conditional envelope in this coordinate.
    pub envelope: ProjectedEnvelope,
}
