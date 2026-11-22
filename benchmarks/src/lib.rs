//! Shared benchmark helpers for `maplibre-rs`.
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
#![deny(unused_imports)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
