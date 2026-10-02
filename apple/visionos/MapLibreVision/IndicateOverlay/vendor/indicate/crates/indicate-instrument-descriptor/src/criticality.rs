//! Derived criticality bands: where a panel's warnings, failure
//! indications, and simulation labelling can land.
//!
//! Ordinary readout surfaces are *declared* — [`crate::PanelDescriptor`]
//! carries `group_regions`. Criticality content has no such declaration
//! and must not gain one: a panel that could name its own warning
//! surface could also understate it. The bound is measured instead, by
//! the admission harness, as the union design-space ink of the
//! `Annunciation` and `Failure` bands over the whole case matrix, and
//! pinned so a consumer holds it as plain data.
//!
//! A band is keyed by panel *and* frame. A panel laid out at two frames
//! puts its warnings in two different places, and a union across frames
//! would be a rectangle in no coordinate space at all.

use crate::descriptor::{DesignFrame, Region};

/// One panel's measured criticality band at one design frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelCriticality {
    /// The panel this bound was measured from.
    pub panel: &'static str,
    /// The design frame it was measured at; the bound is in that
    /// frame's coordinates.
    pub frame: DesignFrame,
    /// The union ink bound, or `None` when the panel put no ink in
    /// either band in any case — an honest empty, not an unknown.
    pub band: Option<Region>,
}

/// The criticality bands a shell holds: pinned constants, or the values
/// an admission run measured and a consumer transcribed.
///
/// Lookup is by panel id and frame together, and a miss is a miss: a
/// consumer asking about a size no run ever measured is told nothing
/// rather than told zero.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CriticalityBands {
    /// The measured entries, in any order; one per panel × frame.
    pub panels: &'static [PanelCriticality],
}

impl CriticalityBands {
    /// No measurements at all. A composition validated against this
    /// refuses every slot, which is the correct answer for a consumer
    /// that pinned nothing.
    pub const EMPTY: CriticalityBands = CriticalityBands { panels: &[] };

    /// The entry for `panel` at `frame`, if one was measured.
    pub fn entry(&self, panel: &str, frame: DesignFrame) -> Option<&'static PanelCriticality> {
        self.panels
            .iter()
            .find(|entry| entry.panel == panel && entry.frame == frame)
    }
}
