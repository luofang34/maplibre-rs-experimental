//! Producer-declared automation alerts and their distinct aural identity.

use crate::{AlertClass, AlertCondition, AlertId, AuralToken, class_of};

/// Flight guidance events declared by the controlling system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AutoflightFault {
    /// Autopilot disconnect requiring crew awareness and response.
    AutopilotDisconnect = 1,
    /// Automatic thrust disconnect requiring crew awareness.
    AutothrustDisconnect = 2,
    /// Active low-speed protection requiring crew awareness.
    LowSpeedProtection = 3,
    /// Active high-speed protection requiring crew awareness.
    HighSpeedProtection = 4,
    /// Unexpected mode reversion requiring crew awareness.
    ModeReversion = 5,
}

impl AutoflightFault {
    pub(crate) const fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::AutopilotDisconnect),
            2 => Some(Self::AutothrustDisconnect),
            3 => Some(Self::LowSpeedProtection),
            4 => Some(Self::HighSpeedProtection),
            5 => Some(Self::ModeReversion),
            _ => None,
        }
    }

    pub(crate) const fn class(self) -> AlertClass {
        match self {
            Self::AutopilotDisconnect => AlertClass::Warning,
            _ => AlertClass::Caution,
        }
    }
}

/// Aural identity for a known condition, including the distinct AP disconnect warning.
pub const fn aural_of(id: AlertId) -> Option<AuralToken> {
    if id.0
        == AlertCondition::Autoflight(AutoflightFault::AutopilotDisconnect)
            .id()
            .0
    {
        return Some(AuralToken::AutopilotDisconnect);
    }
    match class_of(id) {
        Some(class) => Some(class.aural_token()),
        None => None,
    }
}

#[cfg(test)]
mod tests;
