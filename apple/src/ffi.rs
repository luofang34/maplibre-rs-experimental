//! C ABI entrypoints exported by the Apple XCFramework SDK.

use std::{
    ffi::{c_char, CStr},
    panic::{catch_unwind, AssertUnwindSafe},
    path::PathBuf,
};

use crate::{
    init_apple_logging, retain_c_abi_symbols, run_apple_map, AppleMapConfig, AppleSdkError,
    FfiError,
};

#[expect(
    unsafe_code,
    reason = "C FFI string validation reads caller-supplied NUL-terminated pointers"
)]
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
#[expect(
    unsafe_code,
    reason = "Public C FFI helper validates raw C string pointers from Swift/ObjC"
)]
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
#[expect(unsafe_code, reason = "C ABI symbol exported for Xcode host linkage")]
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
#[expect(
    unsafe_code,
    reason = "C ABI entrypoint accepts raw C string pointers from Swift or Objective-C"
)]
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
