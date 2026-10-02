//! Shared guidance, speed coordinates, and conditional envelope presentation.

pub mod envelope;
pub mod guidance;
pub mod presentation;
pub mod speed;
mod validation;

use crate::{ApModes, ApTargetsResolved, ResolvedDirector, Sig, SignalStatus, Stamped};
use guidance::GuidanceSample;
use speed::SpeedSample;

/// Optional producer reports for shared flight presentation.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FlightInputs {
    /// Coherent engagement, mode, target, and command report.
    pub guidance: Stamped<GuidanceSample>,
    /// Airspeed values and model-supplied projections.
    pub speed: Stamped<SpeedSample>,
}

/// Flight inputs resolved once for every instrument in a frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlightDisplayData {
    /// Legacy attitude command report.
    pub director: ResolvedDirector,
    /// Legacy autopilot mode report.
    pub ap_modes: Sig<ApModes>,
    /// Legacy selected targets.
    pub ap_targets: ApTargetsResolved,
    /// Extended guidance report, when supplied.
    pub guidance: Sig<GuidanceSample>,
    /// Extended speed report, when supplied.
    pub speed: Sig<SpeedSample>,
    /// Envelope status independent of measured airspeed.
    pub envelope_status: SignalStatus,
    /// Presence prevents fallback through an invalid extended report.
    pub guidance_present: bool,
    /// Presence prevents fallback through an invalid extended speed report.
    pub speed_present: bool,
    /// Acquisition age used for transition timing.
    pub guidance_age_ms: Option<f32>,
}

/// Identity of one guidance computation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SolutionId {
    /// Nonzero producer identity.
    pub source: u32,
    /// Producer incarnation within the transport session.
    pub epoch: u32,
    /// Wrapping computation sequence.
    pub sequence: u32,
}

impl SolutionId {
    /// Whether the producer identity was declared.
    pub const fn is_declared(self) -> bool {
        self.source != 0
    }
}

#[cfg(test)]
use validation::envelope_fault;
pub(crate) use validation::{guidance_fault, speed_envelope_fault, speed_fault};

#[cfg(test)]
mod tests;
