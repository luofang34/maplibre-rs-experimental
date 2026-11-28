//! Typed resources, tile components and the systems that update a map's world.

#![deny(missing_docs)]

pub use maplibre_tcs::{disjoint, pyramid, resources};

pub mod frame_input;
pub mod geometry_index;
pub mod memory_budget;
pub(crate) mod paint_zoom;
pub(crate) mod property_interner;
pub(crate) mod tile_epochs;
pub(crate) mod tile_window;
pub mod tiles;
pub mod world;
