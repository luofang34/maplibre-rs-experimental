//! Runtime style, GeoJSON, `feature-state`, and sprite image C ABI exports.

#![allow(unsafe_code)]

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
        // SAFETY: Caller upholds `map` and `style_json` pointer validity.
        let map_ref = unsafe { borrow_map_mut(map) }?;
        // SAFETY: Validated against null; caller guarantees NUL-terminated string.
        let style_str = unsafe { required_c_str(style_json, "style_json") }?;
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
        // SAFETY: Caller upholds pointer validity for the duration of the call.
        let map_ref = unsafe { borrow_map_mut(map) }?;
        // SAFETY: Validated against null; caller guarantees NUL-terminated strings.
        let layer = unsafe { required_c_str(layer_id, "layer_id") }?;
        // SAFETY: Validated against null; caller guarantees NUL-terminated strings.
        let prop = unsafe { required_c_str(property_name, "property_name") }?;
        // SAFETY: Validated against null; caller guarantees NUL-terminated strings.
        let value = unsafe { required_c_str(value_json, "value_json") }?;
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
        // SAFETY: Caller upholds pointer validity for the duration of the call.
        let map_ref = unsafe { borrow_map_mut(map) }?;
        // SAFETY: Validated against null; caller guarantees NUL-terminated strings.
        let layer = unsafe { required_c_str(layer_id, "layer_id") }?;
        // SAFETY: Validated against null; caller guarantees NUL-terminated strings.
        let prop = unsafe { required_c_str(property_name, "property_name") }?;
        // SAFETY: Validated against null; caller guarantees NUL-terminated strings.
        let value = unsafe { required_c_str(value_json, "value_json") }?;
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
        // SAFETY: Caller upholds pointer validity for the duration of the call.
        let map_ref = unsafe { borrow_map_mut(map) }?;
        // SAFETY: Validated against null; caller guarantees NUL-terminated string.
        let layer = unsafe { required_c_str(layer_id, "layer_id") }?;
        // SAFETY: Optional C string; `NULL` maps to `None`.
        let filter = unsafe { optional_c_str(filter_json, "filter_json") }?;
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
        // SAFETY: Caller upholds pointer validity for the duration of the call.
        let map_ref = unsafe { borrow_map_mut(map) }?;
        // SAFETY: Validated against null; caller guarantees NUL-terminated string.
        let layer = unsafe { required_c_str(layer_id, "layer_id") }?;
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
        // SAFETY: Caller upholds pointer validity for the duration of the call.
        let map_ref = unsafe { borrow_map_mut(map) }?;
        // SAFETY: Validated against null; caller guarantees NUL-terminated strings.
        let source = unsafe { required_c_str(source_id, "source_id") }?;
        // SAFETY: Validated against null; caller guarantees NUL-terminated strings.
        let geojson = unsafe { required_c_str(geojson_json, "geojson_json") }?;
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
        // SAFETY: Caller upholds pointer validity for the duration of the call.
        let map_ref = unsafe { borrow_map_mut(map) }?;
        // SAFETY: Validated against null; caller guarantees NUL-terminated strings.
        let src = unsafe { required_c_str(source, "source") }?;
        // SAFETY: Optional C string; `NULL` maps to `None`.
        let src_layer = unsafe { optional_c_str(source_layer, "source_layer") }?;
        // SAFETY: Validated against null; caller guarantees NUL-terminated strings.
        let feat_id = unsafe { required_c_str(feature_id, "feature_id") }?;
        // SAFETY: Validated against null; caller guarantees NUL-terminated strings.
        let state = unsafe { required_c_str(state_json, "state_json") }?;
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
        // SAFETY: Caller upholds pointer validity for the duration of the call.
        let map_ref = unsafe { borrow_map_mut(map) }?;
        // SAFETY: Validated against null; caller guarantees NUL-terminated string.
        let src = unsafe { required_c_str(source, "source") }?;
        // SAFETY: Optional C strings; `NULL` maps to `None`.
        let src_layer = unsafe { optional_c_str(source_layer, "source_layer") }?;
        // SAFETY: Optional C strings; `NULL` maps to `None`.
        let feat_id = unsafe { optional_c_str(feature_id, "feature_id") }?;
        // SAFETY: Optional C strings; `NULL` maps to `None`.
        let key = unsafe { optional_c_str(state_key, "state_key") }?;
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
        // SAFETY: Caller upholds pointer validity for the duration of the call.
        let map_ref = unsafe { borrow_map_mut(map) }?;
        // SAFETY: Validated against null; caller guarantees NUL-terminated string.
        let id = unsafe { required_c_str(image_id, "image_id") }?;
        if rgba.is_null() {
            return Err(FfiError::NullPointer("rgba"));
        }
        // SAFETY: `rgba` is non-null and caller guarantees `rgba_len` readable bytes.
        let bytes = unsafe { std::slice::from_raw_parts(rgba, rgba_len) };
        map_ref
            .engine
            .add_image_rgba8(id, width, height, bytes, pixel_ratio, sdf)
    })
}
