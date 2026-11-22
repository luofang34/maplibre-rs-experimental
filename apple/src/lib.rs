//! Embeddable iOS & macOS SDK (`MapLibreRs.xcframework`), re-exporting the complete
//! `maplibre-ffi` C ABI alongside configurable headed Apple entrypoints.

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
#![cfg_attr(
    test,
    allow(
        unsafe_code,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing
    )
)]

pub use maplibre_ffi::*;

mod config;
mod ffi;
#[cfg(test)]
mod tests;

pub use config::{init_apple_logging, run_apple_map, AppleMapConfig, AppleSdkError};
pub use ffi::{build_apple_config_from_c, maplibre_apple_main, maplibre_apple_run_with_style_json};

#[cfg(not(any(no_pendantic_os_check, target_os = "macos", target_os = "ios")))]
compile_error!("apple works only on macOS and iOS.");
