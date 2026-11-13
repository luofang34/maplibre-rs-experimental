//! Frame rendering, redraw polling, and rendered-feature query C ABI exports.

#![allow(unsafe_code)]

use std::ffi::{c_char, CString};

use crate::{
    error::{ffi_try_bool, ffi_try_ptr, ffi_try_value, optional_c_str},
    lifecycle::{borrow_map, borrow_map_mut, MaplibreMap},
    types::FfiError,
};

/// Returns `true` when `map` has dirty state, camera motion, or arrived/pending tile work.
///
/// # Safety
///
/// `map` must point to a live [`MaplibreMap`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maplibre_map_needs_redraw(map: *const MaplibreMap) -> bool {
    ffi_try_value(false, || {
        // SAFETY: Caller upholds `map` pointer validity.
        let map_ref = unsafe { borrow_map(map) }?;
        map_ref.engine.needs_redraw()
    })
}

/// Renders a live/settled frame and copies tightly packed RGBA8 pixels into `out_rgba`
/// (`out_rgba_len` must be at least `width * height * 4`).
///
/// # Safety
///
/// `map` must point to a live [`MaplibreMap`] and `out_rgba` must point to at least
/// `out_rgba_len` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maplibre_map_render_rgba8(
    map: *mut MaplibreMap,
    out_rgba: *mut u8,
    out_rgba_len: usize,
) -> bool {
    ffi_try_bool(|| {
        // SAFETY: Caller upholds `map` pointer validity.
        let map_ref = unsafe { borrow_map_mut(map) }?;
        if out_rgba.is_null() {
            return Err(FfiError::NullPointer("out_rgba"));
        }
        let (width, height) = map_ref.engine.dimensions();
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|px| px.checked_mul(4))
            .ok_or(FfiError::InvalidSize { width, height })?;
        if out_rgba_len < expected {
            return Err(FfiError::InvalidBufferLength {
                expected,
                actual: out_rgba_len,
            });
        }
        let pixels = map_ref.engine.render_rgba8_vec()?;
        if pixels.len() != expected {
            return Err(FfiError::InvalidBufferLength {
                expected,
                actual: pixels.len(),
            });
        }
        // SAFETY: `out_rgba` is non-null and has at least `expected` writable bytes.
        let out_slice = unsafe { std::slice::from_raw_parts_mut(out_rgba, expected) };
        out_slice.copy_from_slice(&pixels);
        Ok(())
    })
}

/// Queries rendered features intersecting `[min_x, min_y, max_x, max_y]` in screen pixels and
/// returns a heap-allocated NUL-terminated JSON array string freed via
/// [`crate::maplibre_string_free`].
///
/// # Safety
///
/// `map` must point to a live [`MaplibreMap`] and `options_json` must be `NULL` or a valid
/// NUL-terminated UTF-8 JSON object string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maplibre_map_query_rendered_features_json(
    map: *const MaplibreMap,
    min_x: f64,
    min_y: f64,
    max_x: f64,
    max_y: f64,
    options_json: *const c_char,
) -> *mut c_char {
    ffi_try_ptr(|| {
        // SAFETY: Caller upholds `map` pointer validity.
        let map_ref = unsafe { borrow_map(map) }?;
        // SAFETY: Optional C string; `NULL` maps to `None`.
        let opts = unsafe { optional_c_str(options_json, "options_json") }?;
        let json = map_ref
            .engine
            .query_rendered_features_json(min_x, min_y, max_x, max_y, opts)?;
        let c_string = CString::new(json).map_err(|_| FfiError::InteriorNul)?;
        Ok(c_string.into_raw())
    })
}
