//! Offscreen rendering, multi-view composition, readback, and reference capture helpers.

pub mod multi_view;
pub mod readback;
pub mod reference;
mod targets;
pub(super) mod terrain_patch;
pub(crate) mod xr;
