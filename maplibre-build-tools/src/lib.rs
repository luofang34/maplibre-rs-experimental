//! Build-time shader validation and optional MBTiles extraction.
#![deny(unused_imports, missing_docs, unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![deny(clippy::let_underscore_must_use)]

#[cfg(feature = "sqlite")]
pub mod mbtiles;
pub mod wgsl;
