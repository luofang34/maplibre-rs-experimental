//! Flat Mercator projection helpers.

pub mod covering_tiles;

pub use covering_tiles::{covering_tiles, MercatorCoveringError, MercatorCoveringOptions};
