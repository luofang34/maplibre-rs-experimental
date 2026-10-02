//! Panel composition and cross-shell identity (ADR-0029, ADR-0033).
//!
//! A registry is plain data composed by each shell from panel
//! descriptors and validated at init; an out-of-repo panel registers by
//! being named in the shell's composition, never by link-time magic. The
//! descriptor vocabulary itself lives in
//! [`indicate_instrument_descriptor`] and is re-exported here, so a
//! shell needs one dependency to compose and a panel needs neither this
//! crate nor its checks.
//!
//! A shell drawing from one provider crate passes a descriptor slice to
//! [`Registry::new`]. A shell drawing from several names
//! [`PanelSet`]s and calls [`Registry::from_sets`], so adding an
//! instrument family is one declaration naming the set rather than one
//! line per panel — the by-hand panel list ADR-0029 removed does not
//! reappear one layer up. Both compositions are `Copy` and
//! allocation-free, and the same per-panel rules run over the flattened
//! traversal either way, so a panel id claimed by two sets is refused
//! at init.

#![no_std]

#[cfg(test)]
extern crate std;

mod composition;
mod digest;
mod registry;

pub use composition::{
    COMPOSITION_DIGEST_DOMAIN, CompositionDescriptor, CompositionError, MAX_COMPOSITION_SLOTS,
    Slot, composition_digest, validate_composition,
};
pub use digest::{DigestError, SCENE_DIGEST_DOMAIN, scene_digest};
pub use indicate_instrument_descriptor::states;
pub use indicate_instrument_descriptor::{
    BackgroundCapability, CANONICAL_STATES, CONFIG_BLOB_MAX, CanonicalState, ConfigBlob,
    ConfigError, ConfigKey, CriticalityBands, DesignFrame, DrawFn, EMPTY_CONFIG, ExtremeState,
    GroupSet, PanelCriticality, PanelDescriptor, PanelDrawError, PanelSet, Region, keys,
};
pub use registry::{Panels, Registry, RegistryError};
