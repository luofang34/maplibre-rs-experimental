//! Panel sets: the unit a shell names when it composes a registry.
//!
//! A provider crate exports one [`PanelSet`] rather than loose
//! descriptors, so adding an instrument family to a shell is one
//! declaration naming the set instead of one line per panel. The
//! registry still validates panels, so a set cannot smuggle a
//! malformed descriptor past the checks by arriving in a group, and
//! cross-set duplicate ids are caught at init rather than resolving to
//! whichever panel the shell happened to list first.
//!
//! Sets are packaging, not paint: set identity stays out of the scene
//! digest, so regrouping the same panels into different sets without
//! reordering them leaves cross-shell identity untouched.

use crate::descriptor::PanelDescriptor;

/// A named group of panels contributed by one provider crate.
#[derive(Debug, Clone, Copy)]
pub struct PanelSet {
    /// Set identity, under the same charset rule as panel ids.
    pub id: &'static str,
    /// The set's panels, in the order they compose.
    pub panels: &'static [PanelDescriptor],
}
