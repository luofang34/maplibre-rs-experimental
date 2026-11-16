//! Typed resources, tile components and the systems that update a map's world.

#![deny(missing_docs)]

pub use maplibre_tcs::{disjoint, pyramid, resources};

pub mod system;
pub mod tiles;
pub mod world;
