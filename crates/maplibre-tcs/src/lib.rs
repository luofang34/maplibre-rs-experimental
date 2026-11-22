//! Typed resources, hierarchical tile pyramids, and disjoint tile component storage.

#![cfg_attr(
    not(test),
    forbid(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::todo,
        clippy::unimplemented
    )
)]
#![forbid(unsafe_code)]
#![deny(dead_code, unused_imports, missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod disjoint;
pub mod heap;
pub mod pyramid;
pub mod resources;
pub mod tiles;
pub mod world;
