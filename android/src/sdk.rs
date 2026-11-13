//! Embeddable offscreen `NativeMap` engine backing Kotlin/Java `Bitmap` & `Surface` views.

#![allow(unsafe_code)]

use jni::sys::jlong;
use maplibre::map::handle::SyncHeadlessHandle;
use maplibre_ffi::{CameraState, FfiError, MapEngine, MapEngineConfig};

use crate::options::AndroidSdkError;

/// Embeddable Android map instance backed by [`MapEngine`] and [`SyncHeadlessHandle`].
///
/// Multiple `NativeMap` instances can coexist in a single process, accept concurrent mutations
/// from Kotlin coroutines or background threads, and render RGBA8 frames for `android.graphics.Bitmap`
/// or `Surface` presentation.
pub struct NativeMap {
    engine: MapEngine,
}

impl NativeMap {
    /// Creates an embeddable offscreen map with its own worker runtime thread.
    pub fn create(config: MapEngineConfig, style_json: &str) -> Result<Self, AndroidSdkError> {
        Ok(Self {
            engine: MapEngine::create(config, style_json)?,
        })
    }

    /// Transfers ownership into a non-zero JNI `jlong` handle freed by [`Self::destroy_raw_handle`].
    pub fn into_raw_handle(self) -> jlong {
        Box::into_raw(Box::new(self)) as usize as jlong
    }

    /// Borrows a live [`NativeMap`] from a JNI `jlong` handle.
    ///
    /// # Safety
    ///
    /// `raw` must be `0` (which returns `Err(InvalidHandle)`) or a live pointer previously returned
    /// by [`Self::into_raw_handle`] that has not been destroyed.
    pub unsafe fn borrow_raw<'a>(raw: jlong) -> Result<&'a Self, AndroidSdkError> {
        if raw == 0 {
            return Err(AndroidSdkError::InvalidHandle);
        }
        let ptr = (raw as usize) as *const Self;
        if !ptr.is_aligned() {
            return Err(AndroidSdkError::InvalidHandle);
        }
        // SAFETY: Caller guarantees non-zero `raw` came from `NativeMap::into_raw_handle`.
        unsafe { ptr.as_ref() }.ok_or(AndroidSdkError::InvalidHandle)
    }

    /// Destroys a [`NativeMap`] previously converted via [`Self::into_raw_handle`]; `0` is ignored.
    ///
    /// # Safety
    ///
    /// `raw` must be `0` or a pointer returned by [`Self::into_raw_handle`] not yet freed.
    pub unsafe fn destroy_raw_handle(raw: jlong) -> Result<(), AndroidSdkError> {
        if raw == 0 {
            return Ok(());
        }
        let ptr = (raw as usize) as *mut Self;
        if !ptr.is_aligned() {
            return Err(AndroidSdkError::InvalidHandle);
        }
        // SAFETY: Caller guarantees `raw` was allocated by `Box::into_raw` in `into_raw_handle`.
        drop(unsafe { Box::from_raw(ptr) });
        Ok(())
    }

    /// Clones a thread-safe [`SyncHeadlessHandle`] for queuing commands from any thread.
    pub fn clone_handle(&self) -> Result<SyncHeadlessHandle, AndroidSdkError> {
        Ok(self.engine.clone_handle()?)
    }

    /// Current `(width, height)` in physical pixels.
    pub fn dimensions(&self) -> (u32, u32) {
        self.engine.dimensions()
    }

    /// Resizes the offscreen target and updates `pixel_ratio`.
    pub fn resize(&self, width: u32, height: u32, pixel_ratio: f64) -> Result<(), AndroidSdkError> {
        Ok(self.engine.resize(width, height, pixel_ratio)?)
    }

    /// Jumps the map camera immediately.
    pub fn set_camera(&self, camera: CameraState) -> Result<(), AndroidSdkError> {
        Ok(self.engine.set_camera(camera)?)
    }

    /// Reads the current camera state.
    pub fn camera(&self) -> Result<CameraState, AndroidSdkError> {
        Ok(self.engine.camera()?)
    }

    /// Replaces the entire MapLibre Style JSON while preserving camera state.
    pub fn set_style_json(&self, style_json: &str) -> Result<(), AndroidSdkError> {
        Ok(self.engine.set_style_json(style_json)?)
    }

    /// Updates a paint property on `layer_id` from JSON (`"null"` restores default).
    pub fn set_paint_property_json(
        &self,
        layer_id: &str,
        property_name: &str,
        value_json: &str,
    ) -> Result<(), AndroidSdkError> {
        Ok(self
            .engine
            .set_paint_property_json(layer_id, property_name, value_json)?)
    }

    /// Replaces the inline GeoJSON data of `source_id`.
    pub fn set_geojson_source(
        &self,
        source_id: &str,
        geojson_json: &str,
    ) -> Result<(), AndroidSdkError> {
        Ok(self
            .engine
            .set_geojson_source_data(source_id, geojson_json)?)
    }

    /// Merges JSON object `state_json` into `feature_id`'s runtime `feature-state`.
    pub fn set_feature_state_json(
        &self,
        source: &str,
        source_layer: Option<&str>,
        feature_id: &str,
        state_json: &str,
    ) -> Result<(), AndroidSdkError> {
        Ok(self
            .engine
            .set_feature_state_json(source, source_layer, feature_id, state_json)?)
    }

    /// Returns `true` when dirty state, camera motion, or tile work requires another frame.
    pub fn needs_redraw(&self) -> Result<bool, AndroidSdkError> {
        Ok(self.engine.needs_redraw()?)
    }

    /// Renders a frame and returns tightly packed RGBA8 bytes (`width * height * 4`).
    pub fn render_rgba8_vec(&self) -> Result<Vec<u8>, AndroidSdkError> {
        Ok(self.engine.render_rgba8_vec()?)
    }

    /// Renders a frame into `out_rgba` (`out_rgba.len() >= width * height * 4`).
    pub fn render_rgba8_into(&self, out_rgba: &mut [u8]) -> Result<(), AndroidSdkError> {
        let (width, height) = self.dimensions();
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|px| px.checked_mul(4))
            .ok_or(FfiError::InvalidSize { width, height })?;
        if out_rgba.len() < expected {
            return Err(AndroidSdkError::Ffi(FfiError::InvalidBufferLength {
                expected,
                actual: out_rgba.len(),
            }));
        }
        let pixels = self.engine.render_rgba8_vec()?;
        out_rgba[..expected].copy_from_slice(&pixels[..expected]);
        Ok(())
    }
}
