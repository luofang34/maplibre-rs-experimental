//! The panel descriptor vocabulary: what a panel declares about itself
//! (ADR-0029, ADR-0033).
//!
//! A panel is a plugin over three stable contracts — the state-group
//! vocabulary, the scene-command IR, and the glyph vocabulary. This
//! crate holds the words a panel uses to describe itself against them:
//! identity, required layers and state groups, the range of design
//! frames it lays out against, background capability, the bounded
//! key-TLV configuration schema, the draw entry point, the panel-set
//! grouping, and the canonical state corpus every panel is expected to
//! survive.
//!
//! Declaring is separate from composing. A panel needs the vocabulary to
//! be describable at all, so the vocabulary is `no_std` and sits low
//! enough that a panel may depend on it. Composing descriptors into a
//! registry, validating them against each other, and hashing the result
//! into a cross-shell identity are a consumer's concerns and live
//! downstream, where a panel never reaches.

#![no_std]

#[cfg(test)]
extern crate std;

mod config;
mod criticality;
mod descriptor;
mod frame;
mod group_set;
mod set;
pub mod states;

pub use config::{CONFIG_BLOB_MAX, ConfigBlob, ConfigError, ConfigKey, EMPTY_CONFIG, keys};
pub use criticality::{CriticalityBands, PanelCriticality};
pub use descriptor::{
    BackgroundCapability, DesignFrame, DrawFn, ExtremeState, PanelDescriptor, PanelDrawError,
    Region,
};
pub use frame::{FRAME_STEP_TOLERANCE, FrameRefusal};
pub use group_set::GroupSet;
pub use set::PanelSet;
pub use states::{CANONICAL_STATES, CanonicalState};
