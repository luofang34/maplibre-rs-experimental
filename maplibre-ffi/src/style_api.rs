//! Runtime style, GeoJSON, `feature-state`, and sprite image C ABI exports.

#![expect(
    unsafe_code,
    reason = "C ABI module borrows raw map pointers and NUL-terminated C strings"
)]

use std::ffi::c_char;

use crate::{
    error::{ffi_try_bool, optional_c_str, required_c_str},
    lifecycle::{borrow_map_mut, MaplibreMap},
    types::FfiError,
};

/// Replaces `map`'s style document with `style_json` while retaining the camera.
///
/// # Safety
///
/// `map` must point to a live [`MaplibreMap`] and `style_json` to a NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maplibre_map_set_style_json(
    map: *mut MaplibreMap,
    style_json: *const c_char,
) -> bool {
    ffi_try_bool(|| {
        // SAFETY: Caller guarantees `map` is live and `style_json` is NUL-terminated.
        let (map_ref, style_str) = unsafe {
            (
                borrow_map_mut(map)?,
                required_c_str(style_json, "style_json")?,
            )
        };
        map_ref.engine.set_style_json(style_str)
    })
}

/// Sets a paint property on `layer_id` from JSON `value_json` (`"null"` restores default).
///
/// # Safety
///
/// `map` must point to a live [`MaplibreMap`] and string arguments to NUL-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maplibre_map_set_paint_property_json(
    map: *mut MaplibreMap,
    layer_id: *const c_char,
    property_name: *const c_char,
    value_json: *const c_char,
) -> bool {
    ffi_try_bool(|| {
        // SAFETY: Caller guarantees `map` and string pointers remain valid for this call.
        let (map_ref, layer, prop, value) = unsafe {
            (
                borrow_map_mut(map)?,
                required_c_str(layer_id, "layer_id")?,
                required_c_str(property_name, "property_name")?,
                required_c_str(value_json, "value_json")?,
            )
        };
        map_ref.engine.set_paint_property_json(layer, prop, value)
    })
}

/// Sets a layout property on `layer_id` from JSON `value_json` (`"null"` restores default).
///
/// # Safety
///
/// `map` must point to a live [`MaplibreMap`] and string arguments to NUL-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maplibre_map_set_layout_property_json(
    map: *mut MaplibreMap,
    layer_id: *const c_char,
    property_name: *const c_char,
    value_json: *const c_char,
) -> bool {
    ffi_try_bool(|| {
        // SAFETY: Caller guarantees `map` and string pointers remain valid for this call.
        let (map_ref, layer, prop, value) = unsafe {
            (
                borrow_map_mut(map)?,
                required_c_str(layer_id, "layer_id")?,
                required_c_str(property_name, "property_name")?,
                required_c_str(value_json, "value_json")?,
            )
        };
        map_ref.engine.set_layout_property_json(layer, prop, value)
    })
}

/// Sets or clears (`NULL` or `"null"`) a filter expression on `layer_id`.
///
/// # Safety
///
/// `map` must point to a live [`MaplibreMap`], `layer_id` to a NUL-terminated C string, and
/// `filter_json` must be `NULL` or a NUL-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maplibre_map_set_filter_json(
    map: *mut MaplibreMap,
    layer_id: *const c_char,
    filter_json: *const c_char,
) -> bool {
    ffi_try_bool(|| {
        // SAFETY: Caller guarantees `map` and string pointers (or null optional filter) remain valid.
        let (map_ref, layer, filter) = unsafe {
            (
                borrow_map_mut(map)?,
                required_c_str(layer_id, "layer_id")?,
                optional_c_str(filter_json, "filter_json")?,
            )
        };
        map_ref.engine.set_filter_json(layer, filter)
    })
}

/// Toggles `layer_id` layout `visibility` between `"visible"` (`true`) and `"none"` (`false`).
///
/// # Safety
///
/// `map` must point to a live [`MaplibreMap`] and `layer_id` to a NUL-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maplibre_map_set_layer_visibility(
    map: *mut MaplibreMap,
    layer_id: *const c_char,
    visible: bool,
) -> bool {
    ffi_try_bool(|| {
        // SAFETY: Caller guarantees `map` and `layer_id` remain valid for this call.
        let (map_ref, layer) =
            unsafe { (borrow_map_mut(map)?, required_c_str(layer_id, "layer_id")?) };
        map_ref.engine.set_layer_visibility(layer, visible)
    })
}

/// Replaces the inline GeoJSON document of `source_id` with `geojson_json`.
///
/// # Safety
///
/// `map` must point to a live [`MaplibreMap`] and string arguments to NUL-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maplibre_map_set_geojson_source_data(
    map: *mut MaplibreMap,
    source_id: *const c_char,
    geojson_json: *const c_char,
) -> bool {
    ffi_try_bool(|| {
        // SAFETY: Caller guarantees `map` and string pointers remain valid for this call.
        let (map_ref, source, geojson) = unsafe {
            (
                borrow_map_mut(map)?,
                required_c_str(source_id, "source_id")?,
                required_c_str(geojson_json, "geojson_json")?,
            )
        };
        map_ref.engine.set_geojson_source_data(source, geojson)
    })
}

/// Merges JSON object `state_json` into `feature_id`'s runtime `feature-state`.
///
/// # Safety
///
/// `map` must point to a live [`MaplibreMap`]; required string pointers must be valid
/// NUL-terminated C strings (`source_layer` may be `NULL`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maplibre_map_set_feature_state_json(
    map: *mut MaplibreMap,
    source: *const c_char,
    source_layer: *const c_char,
    feature_id: *const c_char,
    state_json: *const c_char,
) -> bool {
    ffi_try_bool(|| {
        // SAFETY: Caller guarantees `map` and string pointers (or null `source_layer`) remain valid.
        let (map_ref, src, src_layer, feat_id, state) = unsafe {
            (
                borrow_map_mut(map)?,
                required_c_str(source, "source")?,
                optional_c_str(source_layer, "source_layer")?,
                required_c_str(feature_id, "feature_id")?,
                required_c_str(state_json, "state_json")?,
            )
        };
        map_ref
            .engine
            .set_feature_state_json(src, src_layer, feat_id, state)
    })
}

/// Removes runtime `feature-state` entries in `source` / optional `source_layer`.
///
/// # Safety
///
/// `map` must point to a live [`MaplibreMap`], `source` to a NUL-terminated C string, and
/// `source_layer`, `feature_id`, and `state_key` must each be `NULL` or a NUL-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maplibre_map_remove_feature_state(
    map: *mut MaplibreMap,
    source: *const c_char,
    source_layer: *const c_char,
    feature_id: *const c_char,
    state_key: *const c_char,
) -> bool {
    ffi_try_bool(|| {
        // SAFETY: Caller guarantees `map`, `source`, and any non-null optional pointers remain valid.
        let (map_ref, src, src_layer, feat_id, key) = unsafe {
            (
                borrow_map_mut(map)?,
                required_c_str(source, "source")?,
                optional_c_str(source_layer, "source_layer")?,
                optional_c_str(feature_id, "feature_id")?,
                optional_c_str(state_key, "state_key")?,
            )
        };
        map_ref
            .engine
            .remove_feature_state(src, src_layer, feat_id, key)
    })
}

/// Adds or replaces a named sprite/icon image from straight-alpha RGBA8 pixels.
///
/// # Safety
///
/// `map` must point to a live [`MaplibreMap`], `image_id` to a NUL-terminated C string, and `rgba`
/// must point to at least `rgba_len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maplibre_map_add_image_rgba8(
    map: *mut MaplibreMap,
    image_id: *const c_char,
    width: u32,
    height: u32,
    rgba: *const u8,
    rgba_len: usize,
    pixel_ratio: f32,
    sdf: bool,
) -> bool {
    ffi_try_bool(|| {
        if width == 0 || height == 0 {
            return Err(FfiError::InvalidSize { width, height });
        }
        if !pixel_ratio.is_finite() {
            return Err(FfiError::InvalidNumber("pixel_ratio"));
        }
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|px| px.checked_mul(4))
            .filter(|&n| n <= isize::MAX as usize)
            .ok_or(FfiError::InvalidSize { width, height })?;
        if rgba.is_null() {
            return Err(FfiError::NullPointer("rgba"));
        }
        if rgba_len != expected {
            return Err(FfiError::InvalidBufferLength {
                expected,
                actual: rgba_len,
            });
        }
        // SAFETY: `rgba` is non-null with `expected` readable bytes; caller guarantees pointer validity.
        let (map_ref, id, bytes) = unsafe {
            (
                borrow_map_mut(map)?,
                required_c_str(image_id, "image_id")?,
                std::slice::from_raw_parts(rgba, expected),
            )
        };
        map_ref
            .engine
            .add_image_rgba8(id, width, height, bytes, pixel_ratio, sdf)
    })
}
