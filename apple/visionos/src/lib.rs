//! C ABI of the maplibre-rs renderer for the visionOS host app.
//!
//! The host owns the compositor. Per frame it says where the map's scene stands in its world
//! and, per eye, where the eye is and what it sees. The map draws the eye into its own Metal
//! texture and returns it for the host to copy into the compositor's drawable, and writes
//! the eye's depth straight into the compositor's depth texture.

// C and Metal interop require explicit raw-pointer contracts.
#![allow(unsafe_code)]

mod api;
mod selection;
pub use api::*;
pub use selection::{maplibre_visionos_query_symbols, maplibre_visionos_set_opaque_environment};

mod symbols;
