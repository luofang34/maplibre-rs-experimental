//! Host custom style-layer registration and lifecycle C ABI exports.

#![allow(unsafe_code)]

use std::ffi::{c_char, c_void};

use maplibre::{
    custom_layer::{CustomLayerContext, CustomStyleLayer},
    render::upload_queue::UploadQueue,
};

use crate::{
    error::{ffi_try_bool, ffi_try_value, required_c_str},
    lifecycle::{borrow_map_mut, MaplibreMap},
    types::FfiError,
};

/// C callback table passed to [`maplibre_map_register_custom_layer`].
///
/// Any optional function pointer may be `NULL`. Callbacks execute on the map's internal render
/// thread; `user_data` must remain valid until [`Self::deinitialize`] completes (or until
/// `maplibre_map_destroy` / `maplibre_map_remove_custom_layer` if uninitialized).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MaplibreCustomLayerCallbacks {
    /// Opaque host context pointer forwarded to every callback.
    pub user_data: *mut c_void,
    /// Called before the first frame (or after target format / MSAA changes) to create resources.
    pub initialize:
        Option<unsafe extern "C" fn(user_data: *mut c_void, color_format: u32, sample_count: u32)>,
    /// Called each visible frame during queue preparation before render-pass encoding.
    pub prepare: Option<
        unsafe extern "C" fn(
            user_data: *mut c_void,
            viewport_width: u32,
            viewport_height: u32,
            zoom: f64,
        ),
    >,
    /// Called during the layer's painter-order draw step.
    pub render: Option<
        unsafe extern "C" fn(
            user_data: *mut c_void,
            viewport_width: u32,
            viewport_height: u32,
            zoom: f64,
        ),
    >,
    /// Called once per `initialize` cycle when removed, replaced, torn down by style edits, or destroyed.
    pub deinitialize: Option<unsafe extern "C" fn(user_data: *mut c_void)>,
}

struct FfiCustomLayer {
    id: String,
    callbacks: MaplibreCustomLayerCallbacks,
}

// SAFETY: `MaplibreMap` serializes every callback onto `HeadlessRuntimeThread`.
unsafe impl Send for FfiCustomLayer {}
// SAFETY: `MaplibreMap` invokes `CustomStyleLayer` solely from `HeadlessRuntimeThread`.
unsafe impl Sync for FfiCustomLayer {}

const fn color_format_tag(format: wgpu::TextureFormat) -> u32 {
    match format {
        wgpu::TextureFormat::Rgba8Unorm => 1,
        wgpu::TextureFormat::Bgra8Unorm => 2,
        wgpu::TextureFormat::Rgba8UnormSrgb => 3,
        wgpu::TextureFormat::Bgra8UnormSrgb => 4,
        _ => 0,
    }
}

impl CustomStyleLayer for FfiCustomLayer {
    fn id(&self) -> &str {
        &self.id
    }

    fn initialize(
        &mut self,
        _device: &wgpu::Device,
        _queue: &UploadQueue,
        color_format: wgpu::TextureFormat,
        sample_count: u32,
    ) {
        if let Some(cb) = self.callbacks.initialize {
            // SAFETY: Caller guarantees `user_data` and `initialize` remain valid while registered.
            unsafe {
                cb(
                    self.callbacks.user_data,
                    color_format_tag(color_format),
                    sample_count,
                )
            };
        }
    }

    fn prepare(&mut self, ctx: &CustomLayerContext<'_>) {
        if let Some(cb) = self.callbacks.prepare {
            // SAFETY: Caller guarantees `user_data` and `prepare` remain valid while registered.
            unsafe {
                cb(
                    self.callbacks.user_data,
                    ctx.viewport[0],
                    ctx.viewport[1],
                    ctx.zoom,
                )
            };
        }
    }

    fn render<'pass>(
        &'pass self,
        _pass: &mut wgpu::RenderPass<'pass>,
        ctx: &CustomLayerContext<'_>,
    ) {
        if let Some(cb) = self.callbacks.render {
            // SAFETY: Caller guarantees `user_data` and `render` remain valid while registered.
            unsafe {
                cb(
                    self.callbacks.user_data,
                    ctx.viewport[0],
                    ctx.viewport[1],
                    ctx.zoom,
                )
            };
        }
    }

    fn deinitialize(&mut self) {
        if let Some(cb) = self.callbacks.deinitialize {
            // SAFETY: Caller guarantees `user_data` remains valid through `deinitialize`.
            unsafe { cb(self.callbacks.user_data) };
        }
    }
}

/// Registers or replaces a host custom style layer matched by `"type": "custom"` in the style.
///
/// Replacing an already-initialized layer with the same `layer_id` immediately invokes the old
/// registration's `deinitialize` callback before registering the new callbacks.
///
/// # Safety
///
/// `map` must point to a live [`MaplibreMap`], `layer_id` to a valid NUL-terminated UTF-8 string,
/// `callbacks` to a valid [`MaplibreCustomLayerCallbacks`], and callback pointers plus
/// `callbacks->user_data` must remain valid until `deinitialize` completes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maplibre_map_register_custom_layer(
    map: *mut MaplibreMap,
    layer_id: *const c_char,
    callbacks: *const MaplibreCustomLayerCallbacks,
) -> bool {
    ffi_try_bool(|| {
        // SAFETY: Caller upholds `map` pointer validity.
        let map_ref = unsafe { borrow_map_mut(map) }?;
        // SAFETY: Validated against null; caller guarantees NUL-terminated UTF-8 string.
        let id = unsafe { required_c_str(layer_id, "layer_id") }?;
        if id.is_empty() {
            return Err(FfiError::EmptyString("layer_id"));
        }
        // SAFETY: Validated against null; caller guarantees valid `MaplibreCustomLayerCallbacks`.
        let cb = unsafe { callbacks.as_ref() }.ok_or(FfiError::NullPointer("callbacks"))?;
        map_ref.engine.register_custom_layer(FfiCustomLayer {
            id: id.to_owned(),
            callbacks: *cb,
        })
    })
}

/// Unregisters the host custom layer registered under `layer_id`.
///
/// Returns `true` when an entry existed (invoking `deinitialize` if initialized), or `false` when
/// no entry existed (with `""` in [`crate::maplibre_last_error_message`]) or on invalid arguments
/// (with a non-empty error message).
///
/// # Safety
///
/// `map` must point to a live [`MaplibreMap`] and `layer_id` to a valid NUL-terminated UTF-8
/// string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maplibre_map_remove_custom_layer(
    map: *mut MaplibreMap,
    layer_id: *const c_char,
) -> bool {
    ffi_try_value(false, || {
        // SAFETY: Caller upholds `map` pointer validity.
        let map_ref = unsafe { borrow_map_mut(map) }?;
        // SAFETY: Validated against null; caller guarantees NUL-terminated UTF-8 string.
        let id = unsafe { required_c_str(layer_id, "layer_id") }?;
        if id.is_empty() {
            return Err(FfiError::EmptyString("layer_id"));
        }
        map_ref.engine.remove_custom_layer(id)
    })
}
