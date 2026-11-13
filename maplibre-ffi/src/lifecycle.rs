//! Map lifecycle, camera, error reporting, and thread-safe handle C ABI exports.

#![allow(unsafe_code)]

use std::ffi::{c_char, CString};

use maplibre::map::handle::{MapClosed, SyncHeadlessHandle};

use crate::{
    engine::MapEngine,
    error::{
        ffi_infallible, ffi_try_bool, ffi_try_ptr, last_error_ptr, optional_c_str, required_c_str,
    },
    types::{
        apply_whole_style, parse_feature_state_args, parse_style_json, CameraState, FfiError,
        MapEngineConfig,
    },
};

const VERSION_NUL: &[u8] = concat!(env!("CARGO_PKG_VERSION"), "\0").as_bytes();

/// Opaque map instance allocated on the Rust heap and owned by a C caller.
pub struct MaplibreMap {
    pub(crate) engine: MapEngine,
}

/// Opaque thread-safe handle (`Send + Sync`) capable of queuing map commands from any thread.
pub struct MaplibreHandle {
    pub(crate) handle: SyncHeadlessHandle,
}

/// Initial map configuration passed to [`maplibre_map_create`].
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MaplibreConfig {
    /// Physical render target width in pixels (must be `> 0`).
    pub width: u32,
    /// Physical render target height in pixels (must be `> 0`).
    pub height: u32,
    /// Physical-to-logical pixel ratio (`<= 0.0` defaults to `1.0`).
    pub pixel_ratio: f64,
    /// Optional NUL-terminated UTF-8 HTTP tile cache directory, or `NULL`.
    pub cache_dir: *const c_char,
    /// Maximum camera pitch in degrees (`<= 0.0` uses default `60.0`).
    pub max_pitch_deg: f64,
}

impl Default for MaplibreConfig {
    fn default() -> Self {
        Self {
            width: 512,
            height: 512,
            pixel_ratio: 1.0,
            cache_dir: std::ptr::null(),
            max_pitch_deg: 0.0,
        }
    }
}

/// Map camera state in geographic degrees and continuous MapLibre zoom.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MaplibreCamera {
    /// Center latitude in degrees.
    pub latitude: f64,
    /// Center longitude in degrees.
    pub longitude: f64,
    /// Continuous MapLibre zoom level.
    pub zoom: f64,
    /// Bearing clockwise from north in degrees.
    pub bearing_deg: f64,
    /// Pitch away from nadir in degrees.
    pub pitch_deg: f64,
}

impl From<CameraState> for MaplibreCamera {
    fn from(value: CameraState) -> Self {
        Self {
            latitude: value.latitude,
            longitude: value.longitude,
            zoom: value.zoom,
            bearing_deg: value.bearing_deg,
            pitch_deg: value.pitch_deg,
        }
    }
}

impl From<MaplibreCamera> for CameraState {
    fn from(value: MaplibreCamera) -> Self {
        Self {
            latitude: value.latitude,
            longitude: value.longitude,
            zoom: value.zoom,
            bearing_deg: value.bearing_deg,
            pitch_deg: value.pitch_deg,
        }
    }
}

pub(crate) unsafe fn borrow_map<'a>(ptr: *const MaplibreMap) -> Result<&'a MaplibreMap, FfiError> {
    // SAFETY: Caller guarantees non-null `ptr` points to a live `MaplibreMap`.
    unsafe { ptr.as_ref() }.ok_or(FfiError::NullPointer("map"))
}

pub(crate) unsafe fn borrow_map_mut<'a>(
    ptr: *mut MaplibreMap,
) -> Result<&'a mut MaplibreMap, FfiError> {
    // SAFETY: Caller guarantees non-null `ptr` points to a live, exclusively borrowed `MaplibreMap`.
    unsafe { ptr.as_mut() }.ok_or(FfiError::NullPointer("map"))
}

unsafe fn borrow_handle<'a>(ptr: *const MaplibreHandle) -> Result<&'a MaplibreHandle, FfiError> {
    // SAFETY: Caller guarantees non-null `ptr` points to a live `MaplibreHandle`.
    unsafe { ptr.as_ref() }.ok_or(FfiError::NullPointer("handle"))
}

/// Returns a static NUL-terminated library version string.
#[unsafe(no_mangle)]
pub extern "C" fn maplibre_version() -> *const c_char {
    VERSION_NUL.as_ptr().cast::<c_char>()
}

/// Returns the thread-local NUL-terminated error message from the most recent `maplibre_*` call
/// on the calling thread (`""` when the most recent call succeeded).
#[unsafe(no_mangle)]
pub extern "C" fn maplibre_last_error_message() -> *const c_char {
    ffi_infallible(c"panic reading thread-local error".as_ptr(), last_error_ptr)
}

/// Frees a heap-allocated C string returned by [`crate::maplibre_map_query_rendered_features_json`].
///
/// # Safety
///
/// `s` must be `NULL` or a pointer returned by a `maplibre_*` string-allocating function that has
/// not yet been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maplibre_string_free(s: *mut c_char) {
    ffi_infallible((), || {
        if !s.is_null() {
            // SAFETY: `s` was allocated via `CString::into_raw` by this library and is freed once.
            drop(unsafe { CString::from_raw(s) });
        }
    });
}

/// Creates a new offscreen map configured by `config` and `style_json`.
///
/// # Safety
///
/// `config` must point to a valid [`MaplibreConfig`] and `style_json` (plus non-null
/// `config.cache_dir`) must point to valid NUL-terminated C strings for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maplibre_map_create(
    config: *const MaplibreConfig,
    style_json: *const c_char,
) -> *mut MaplibreMap {
    ffi_try_ptr(|| {
        // SAFETY: Validated against null; caller guarantees `config` points to a valid struct.
        let cfg = unsafe { config.as_ref() }.ok_or(FfiError::NullPointer("config"))?;
        // SAFETY: Validated against null; caller guarantees NUL-terminated string.
        let style_str = unsafe { required_c_str(style_json, "style_json") }?;
        // SAFETY: Caller guarantees non-null `cache_dir` is a valid NUL-terminated string.
        let cache_dir = unsafe { optional_c_str(cfg.cache_dir, "cache_dir") }?.map(str::to_owned);
        let engine = MapEngine::create(
            MapEngineConfig {
                width: cfg.width,
                height: cfg.height,
                pixel_ratio: cfg.pixel_ratio,
                cache_dir,
                max_pitch_deg: cfg.max_pitch_deg,
            },
            style_str,
        )?;
        Ok(Box::into_raw(Box::new(MaplibreMap { engine })))
    })
}

/// Destroys a map created by [`maplibre_map_create`]; `NULL` is ignored.
///
/// # Safety
///
/// `map` must be `NULL` or a pointer returned by [`maplibre_map_create`] that has not yet been
/// destroyed and has no concurrent borrows.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maplibre_map_destroy(map: *mut MaplibreMap) {
    ffi_infallible((), || {
        if !map.is_null() {
            // SAFETY: `map` was allocated via `Box::into_raw` in `maplibre_map_create`.
            drop(unsafe { Box::from_raw(map) });
        }
    });
}

/// Resizes `map` to `width` x `height` physical pixels and updates `pixel_ratio`.
///
/// # Safety
///
/// `map` must point to a live [`MaplibreMap`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maplibre_map_resize(
    map: *mut MaplibreMap,
    width: u32,
    height: u32,
    pixel_ratio: f64,
) -> bool {
    ffi_try_bool(|| {
        // SAFETY: Caller upholds `map` pointer validity.
        let map_ref = unsafe { borrow_map_mut(map) }?;
        map_ref.engine.resize(width, height, pixel_ratio)
    })
}

/// Clones a thread-safe [`MaplibreHandle`] to `map` usable from any OS thread.
///
/// # Safety
///
/// `map` must point to a live [`MaplibreMap`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maplibre_map_clone_handle(map: *const MaplibreMap) -> *mut MaplibreHandle {
    ffi_try_ptr(|| {
        // SAFETY: Caller upholds `map` pointer validity.
        let map_ref = unsafe { borrow_map(map) }?;
        let handle = map_ref.engine.clone_handle()?;
        Ok(Box::into_raw(Box::new(MaplibreHandle { handle })))
    })
}

/// Destroys a handle created by [`maplibre_map_clone_handle`]; `NULL` is ignored.
///
/// # Safety
///
/// `handle` must be `NULL` or a pointer returned by [`maplibre_map_clone_handle`] not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maplibre_handle_destroy(handle: *mut MaplibreHandle) {
    ffi_infallible((), || {
        if !handle.is_null() {
            // SAFETY: `handle` was allocated via `Box::into_raw` in `maplibre_map_clone_handle`.
            drop(unsafe { Box::from_raw(handle) });
        }
    });
}

/// Queues an asynchronous camera jump from any thread via `handle`.
///
/// # Safety
///
/// `handle` and `camera` must point to valid [`MaplibreHandle`] and [`MaplibreCamera`] values.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maplibre_handle_set_camera(
    handle: *const MaplibreHandle,
    camera: *const MaplibreCamera,
) -> bool {
    ffi_try_bool(|| {
        // SAFETY: Caller upholds `handle` and `camera` pointer validity.
        let handle_ref = unsafe { borrow_handle(handle) }?;
        // SAFETY: Validated against null; caller guarantees `camera` points to `MaplibreCamera`.
        let cam = unsafe { camera.as_ref() }.ok_or(FfiError::NullPointer("camera"))?;
        if handle_ref.handle.is_closed() {
            return Err(FfiError::Closed(MapClosed));
        }
        let options = CameraState::from(*cam).to_options()?;
        handle_ref.handle.send_fire_and_forget(move |map| {
            let _ = map.jump_to(options);
        });
        Ok(())
    })
}

/// Validates `style_json` and queues an asynchronous whole-style replacement from any thread.
///
/// # Safety
///
/// `handle` must point to a valid [`MaplibreHandle`] and `style_json` to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maplibre_handle_set_style_json(
    handle: *const MaplibreHandle,
    style_json: *const c_char,
) -> bool {
    ffi_try_bool(|| {
        // SAFETY: Caller upholds `handle` and `style_json` pointer validity.
        let handle_ref = unsafe { borrow_handle(handle) }?;
        // SAFETY: Validated against null; caller guarantees NUL-terminated string.
        let raw_style = unsafe { required_c_str(style_json, "style_json") }?;
        if handle_ref.handle.is_closed() {
            return Err(FfiError::Closed(MapClosed));
        }
        let parsed_style = parse_style_json(raw_style)?;
        handle_ref.handle.send_fire_and_forget(move |map| {
            let _ = apply_whole_style(map, parsed_style);
        });
        Ok(())
    })
}

/// Validates `state_json` and queues an asynchronous `feature-state` merge from any thread.
///
/// # Safety
///
/// `handle` must point to a valid [`MaplibreHandle`] and string pointers must be `NULL` (where
/// optional) or valid NUL-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maplibre_handle_set_feature_state_json(
    handle: *const MaplibreHandle,
    source: *const c_char,
    source_layer: *const c_char,
    feature_id: *const c_char,
    state_json: *const c_char,
) -> bool {
    ffi_try_bool(|| {
        // SAFETY: Caller upholds `handle` and string pointer validity.
        let handle_ref = unsafe { borrow_handle(handle) }?;
        // SAFETY: Validated against null; caller guarantees NUL-terminated strings.
        let src = unsafe { required_c_str(source, "source") }?;
        // SAFETY: Optional C string; `NULL` maps to `None`.
        let src_layer = unsafe { optional_c_str(source_layer, "source_layer") }?;
        // SAFETY: Validated against null; caller guarantees NUL-terminated strings.
        let feat_id = unsafe { required_c_str(feature_id, "feature_id") }?;
        // SAFETY: Validated against null; caller guarantees NUL-terminated strings.
        let state_raw = unsafe { required_c_str(state_json, "state_json") }?;
        if handle_ref.handle.is_closed() {
            return Err(FfiError::Closed(MapClosed));
        }
        let (target, id, state_map) = parse_feature_state_args(src, src_layer, feat_id, state_raw)?;
        handle_ref.handle.send_fire_and_forget(move |map| {
            map.set_feature_state(target, id, state_map);
        });
        Ok(())
    })
}

/// Reads the map's current camera state into `*out_camera`.
///
/// # Safety
///
/// `map` must point to a live [`MaplibreMap`] and `out_camera` to writable [`MaplibreCamera`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maplibre_map_get_camera(
    map: *const MaplibreMap,
    out_camera: *mut MaplibreCamera,
) -> bool {
    ffi_try_bool(|| {
        // SAFETY: Caller upholds `map` and `out_camera` pointer validity.
        let map_ref = unsafe { borrow_map(map) }?;
        // SAFETY: Validated against null; caller guarantees writable `MaplibreCamera`.
        let out = unsafe { out_camera.as_mut() }.ok_or(FfiError::NullPointer("out_camera"))?;
        *out = MaplibreCamera::from(map_ref.engine.camera()?);
        Ok(())
    })
}

/// Jumps the map's camera to `*camera`.
///
/// # Safety
///
/// `map` must point to a live [`MaplibreMap`] and `camera` to a valid [`MaplibreCamera`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maplibre_map_set_camera(
    map: *mut MaplibreMap,
    camera: *const MaplibreCamera,
) -> bool {
    ffi_try_bool(|| {
        // SAFETY: Caller upholds `map` and `camera` pointer validity.
        let map_ref = unsafe { borrow_map_mut(map) }?;
        // SAFETY: Validated against null; caller guarantees `camera` points to `MaplibreCamera`.
        let cam = unsafe { camera.as_ref() }.ok_or(FfiError::NullPointer("camera"))?;
        map_ref.engine.set_camera(CameraState::from(*cam))
    })
}
