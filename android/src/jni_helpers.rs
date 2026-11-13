//! Shared safe/checked JNI implementation helpers backing `MapLibreSdk` and `NativeMap`.

#![allow(unsafe_code)]

use jni::{
    errors::ThrowRuntimeExAndDefault,
    objects::{JByteArray, JDoubleArray, JString},
    sys::{jboolean, jdouble, jint, jlong},
    Env, EnvUnowned,
};
use maplibre_ffi::{CameraState, FfiError, MapEngineConfig};

use crate::{options::AndroidSdkError, sdk::NativeMap};

fn required_jstring(
    env: &Env<'_>,
    value: &JString<'_>,
    name: &'static str,
) -> Result<String, AndroidSdkError> {
    if value.is_null() {
        return Err(AndroidSdkError::NullArgument(name));
    }
    Ok(value.mutf8_chars(env)?.to_str().into_owned())
}

fn optional_jstring(env: &Env<'_>, value: &JString<'_>) -> Result<Option<String>, AndroidSdkError> {
    if value.is_null() {
        return Ok(None);
    }
    Ok(Some(value.mutf8_chars(env)?.to_str().into_owned()))
}

fn positive_u32(value: jint, dims: (jint, jint)) -> Result<u32, AndroidSdkError> {
    u32::try_from(value).ok().filter(|&v| v > 0).ok_or_else(|| {
        AndroidSdkError::Ffi(FfiError::InvalidSize {
            width: u32::try_from(dims.0).unwrap_or(0),
            height: u32::try_from(dims.1).unwrap_or(0),
        })
    })
}

pub(crate) fn jni_create(
    mut unowned_env: EnvUnowned<'_>,
    width: jint,
    height: jint,
    pixel_ratio: jdouble,
    cache_dir: JString<'_>,
    max_pitch_deg: jdouble,
    style_json: JString<'_>,
) -> jlong {
    unowned_env
        .with_env(|env| -> Result<jlong, AndroidSdkError> {
            let w = positive_u32(width, (width, height))?;
            let h = positive_u32(height, (width, height))?;
            let style = required_jstring(env, &style_json, "styleJson")?;
            let cache = optional_jstring(env, &cache_dir)?;
            let map = NativeMap::create(
                MapEngineConfig {
                    width: w,
                    height: h,
                    pixel_ratio,
                    cache_dir: cache,
                    max_pitch_deg,
                },
                &style,
            )?;
            Ok(map.into_raw_handle())
        })
        .resolve::<ThrowRuntimeExAndDefault>()
}

pub(crate) unsafe fn jni_destroy(mut unowned_env: EnvUnowned<'_>, map_ptr: jlong) {
    unowned_env
        .with_env(|_env| -> Result<(), AndroidSdkError> {
            // SAFETY: Caller passes `0` or a handle produced by `jni_create`.
            unsafe { NativeMap::destroy_raw_handle(map_ptr) }
        })
        .resolve::<ThrowRuntimeExAndDefault>();
}

pub(crate) unsafe fn jni_resize(
    mut unowned_env: EnvUnowned<'_>,
    map_ptr: jlong,
    width: jint,
    height: jint,
    pixel_ratio: jdouble,
) -> jboolean {
    unowned_env
        .with_env(|_env| -> Result<jboolean, AndroidSdkError> {
            // SAFETY: Caller passes a live `NativeMap` handle.
            let map = unsafe { NativeMap::borrow_raw(map_ptr) }?;
            let w = positive_u32(width, (width, height))?;
            let h = positive_u32(height, (width, height))?;
            map.resize(w, h, pixel_ratio)?;
            Ok(true)
        })
        .resolve::<ThrowRuntimeExAndDefault>()
}

pub(crate) unsafe fn jni_set_camera(
    mut unowned_env: EnvUnowned<'_>,
    map_ptr: jlong,
    latitude: jdouble,
    longitude: jdouble,
    zoom: jdouble,
    bearing_deg: jdouble,
    pitch_deg: jdouble,
) -> jboolean {
    unowned_env
        .with_env(|_env| -> Result<jboolean, AndroidSdkError> {
            // SAFETY: Caller passes a live `NativeMap` handle.
            let map = unsafe { NativeMap::borrow_raw(map_ptr) }?;
            map.set_camera(CameraState {
                latitude,
                longitude,
                zoom,
                bearing_deg,
                pitch_deg,
            })?;
            Ok(true)
        })
        .resolve::<ThrowRuntimeExAndDefault>()
}

pub(crate) unsafe fn jni_get_camera<'caller>(
    mut unowned_env: EnvUnowned<'caller>,
    map_ptr: jlong,
) -> JDoubleArray<'caller> {
    unowned_env
        .with_env(|env| -> Result<JDoubleArray<'caller>, AndroidSdkError> {
            // SAFETY: Caller passes a live `NativeMap` handle.
            let map = unsafe { NativeMap::borrow_raw(map_ptr) }?;
            let cam = map.camera()?;
            let out = JDoubleArray::new(env, 5)?;
            out.set_region(
                env,
                0,
                &[
                    cam.latitude,
                    cam.longitude,
                    cam.zoom,
                    cam.bearing_deg,
                    cam.pitch_deg,
                ],
            )?;
            Ok(out)
        })
        .resolve::<ThrowRuntimeExAndDefault>()
}

pub(crate) unsafe fn jni_set_style_json(
    mut unowned_env: EnvUnowned<'_>,
    map_ptr: jlong,
    style_json: JString<'_>,
) -> jboolean {
    unowned_env
        .with_env(|env| -> Result<jboolean, AndroidSdkError> {
            // SAFETY: Caller passes a live `NativeMap` handle.
            let map = unsafe { NativeMap::borrow_raw(map_ptr) }?;
            let style = required_jstring(env, &style_json, "styleJson")?;
            map.set_style_json(&style)?;
            Ok(true)
        })
        .resolve::<ThrowRuntimeExAndDefault>()
}

pub(crate) unsafe fn jni_set_paint_property_json(
    mut unowned_env: EnvUnowned<'_>,
    map_ptr: jlong,
    layer_id: JString<'_>,
    property_name: JString<'_>,
    value_json: JString<'_>,
) -> jboolean {
    unowned_env
        .with_env(|env| -> Result<jboolean, AndroidSdkError> {
            // SAFETY: Caller passes a live `NativeMap` handle.
            let map = unsafe { NativeMap::borrow_raw(map_ptr) }?;
            let layer = required_jstring(env, &layer_id, "layerId")?;
            let prop = required_jstring(env, &property_name, "propertyName")?;
            let value = required_jstring(env, &value_json, "valueJson")?;
            map.set_paint_property_json(&layer, &prop, &value)?;
            Ok(true)
        })
        .resolve::<ThrowRuntimeExAndDefault>()
}

pub(crate) unsafe fn jni_set_geojson_source(
    mut unowned_env: EnvUnowned<'_>,
    map_ptr: jlong,
    source_id: JString<'_>,
    geojson_json: JString<'_>,
) -> jboolean {
    unowned_env
        .with_env(|env| -> Result<jboolean, AndroidSdkError> {
            // SAFETY: Caller passes a live `NativeMap` handle.
            let map = unsafe { NativeMap::borrow_raw(map_ptr) }?;
            let source = required_jstring(env, &source_id, "sourceId")?;
            let geojson = required_jstring(env, &geojson_json, "geojsonJson")?;
            map.set_geojson_source(&source, &geojson)?;
            Ok(true)
        })
        .resolve::<ThrowRuntimeExAndDefault>()
}

pub(crate) unsafe fn jni_set_feature_state_json(
    mut unowned_env: EnvUnowned<'_>,
    map_ptr: jlong,
    source: JString<'_>,
    source_layer: JString<'_>,
    feature_id: JString<'_>,
    state_json: JString<'_>,
) -> jboolean {
    unowned_env
        .with_env(|env| -> Result<jboolean, AndroidSdkError> {
            // SAFETY: Caller passes a live `NativeMap` handle.
            let map = unsafe { NativeMap::borrow_raw(map_ptr) }?;
            let src = required_jstring(env, &source, "source")?;
            let src_layer = optional_jstring(env, &source_layer)?;
            let feat_id = required_jstring(env, &feature_id, "featureId")?;
            let state = required_jstring(env, &state_json, "stateJson")?;
            map.set_feature_state_json(&src, src_layer.as_deref(), &feat_id, &state)?;
            Ok(true)
        })
        .resolve::<ThrowRuntimeExAndDefault>()
}

pub(crate) unsafe fn jni_needs_redraw(mut unowned_env: EnvUnowned<'_>, map_ptr: jlong) -> jboolean {
    unowned_env
        .with_env(|_env| -> Result<jboolean, AndroidSdkError> {
            // SAFETY: Caller passes a live `NativeMap` handle.
            let map = unsafe { NativeMap::borrow_raw(map_ptr) }?;
            map.needs_redraw()
        })
        .resolve::<ThrowRuntimeExAndDefault>()
}

pub(crate) unsafe fn jni_render_rgba8(
    mut unowned_env: EnvUnowned<'_>,
    map_ptr: jlong,
    out_rgba: JByteArray<'_>,
) -> jboolean {
    unowned_env
        .with_env(|env| -> Result<jboolean, AndroidSdkError> {
            if out_rgba.is_null() {
                return Err(AndroidSdkError::NullArgument("outRgba"));
            }
            // SAFETY: Caller passes a live `NativeMap` handle.
            let map = unsafe { NativeMap::borrow_raw(map_ptr) }?;
            let (w, h) = map.dimensions();
            let expected = (w as usize)
                .checked_mul(h as usize)
                .and_then(|px| px.checked_mul(4))
                .ok_or(AndroidSdkError::Ffi(FfiError::InvalidSize {
                    width: w,
                    height: h,
                }))?;
            let actual = out_rgba.len(env)?;
            if actual < expected {
                return Err(AndroidSdkError::Ffi(FfiError::InvalidBufferLength {
                    expected,
                    actual,
                }));
            }
            let pixels = map.render_rgba8_vec()?;
            out_rgba.set_region(env, 0, bytemuck::cast_slice::<u8, i8>(&pixels[..expected]))?;
            Ok(true)
        })
        .resolve::<ThrowRuntimeExAndDefault>()
}
