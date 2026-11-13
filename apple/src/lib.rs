//! Embeddable iOS & macOS SDK (`MapLibreRs.xcframework`), re-exporting the complete
//! `maplibre-ffi` C ABI alongside configurable headed Apple entrypoints.

#![deny(unused_imports)]
#![allow(unexpected_cfgs, unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::{
    ffi::{c_char, CStr},
    panic::{catch_unwind, AssertUnwindSafe},
    path::PathBuf,
};

pub use maplibre_ffi::*;

mod config;
#[cfg(test)]
mod tests;

pub use config::{init_apple_logging, run_apple_map, AppleMapConfig, AppleSdkError};

#[cfg(not(any(no_pendantic_os_check, target_os = "macos", target_os = "ios")))]
compile_error!("apple works only on macOS and iOS.");

unsafe fn optional_utf8<'a>(
    ptr: *const c_char,
    name: &'static str,
) -> Result<Option<&'a str>, FfiError> {
    if ptr.is_null() {
        return Ok(None);
    }
    // SAFETY: Caller guarantees non-null `ptr` points to a NUL-terminated C string.
    let s = unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map_err(|_| FfiError::InvalidUtf8(name))?;
    Ok(Some(s))
}

/// Parses C arguments for [`maplibre_apple_run_with_style_json`] into an [`AppleMapConfig`] and
/// validates its style and numeric parameters before opening a window.
///
/// # Safety
///
/// `style_json` and `cache_dir` must each be `NULL` or point to valid NUL-terminated C strings.
pub unsafe fn build_apple_config_from_c(
    style_json: *const c_char,
    cache_dir: *const c_char,
    globe: bool,
    max_pitch_deg: f64,
) -> Result<AppleMapConfig, AppleSdkError> {
    // SAFETY: Forwarded from C caller upholding string pointer validity.
    let style_opt = unsafe { optional_utf8(style_json, "style_json") }?
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    // SAFETY: Forwarded from C caller upholding string pointer validity.
    let cache_opt = unsafe { optional_utf8(cache_dir, "cache_dir") }?
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    let config = AppleMapConfig {
        style_json: style_opt,
        cache_dir: cache_opt,
        globe,
        max_pitch_deg,
        ..AppleMapConfig::default()
    };
    let _ = config.resolve_style()?;
    let _ = config.headed_options()?;
    Ok(config)
}

/// Default Xcode demo entrypoint launching a headed map with [`AppleMapConfig::default()`].
#[unsafe(no_mangle)]
pub fn maplibre_apple_main() {
    let _ = retain_c_abi_symbols();
    init_apple_logging();
    if let Err(error) = run_apple_map(AppleMapConfig::default()) {
        tracing::error!(%error, ?error, "map startup failed");
    }
}

/// Configurable C ABI entrypoint for launching a headed iOS / macOS map from Swift or Objective-C.
///
/// Pass `NULL` (or `""`) for `style_json` to use the default demo style, and `NULL` for
/// `cache_dir` to disable on-disk HTTP caching. Returns `false` if validation or map execution
/// fails, never unwinding across the C boundary.
///
/// # Safety
///
/// `style_json` and `cache_dir` must each be `NULL` or point to valid NUL-terminated UTF-8 strings
/// for the duration of initialization.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maplibre_apple_run_with_style_json(
    style_json: *const c_char,
    cache_dir: *const c_char,
    globe: bool,
    max_pitch_deg: f64,
) -> bool {
    let _ = retain_c_abi_symbols();
    init_apple_logging();
    match catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: Forwarded from C caller upholding pointer validity.
        let config =
            unsafe { build_apple_config_from_c(style_json, cache_dir, globe, max_pitch_deg) }?;
        run_apple_map(config)
    })) {
        Ok(Ok(())) => true,
        Ok(Err(error)) => {
            tracing::error!(%error, ?error, "apple map run failed");
            false
        }
        Err(_) => false,
    }
}
