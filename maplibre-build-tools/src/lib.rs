//! Build-time shader validation and optional MBTiles extraction.
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
#![deny(unused_imports, missing_docs, unsafe_code)]
#![deny(clippy::panic)]
#![deny(clippy::let_underscore_must_use)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
pub mod mbtiles;
pub mod wgsl;
