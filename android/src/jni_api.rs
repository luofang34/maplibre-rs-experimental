//! JNI exports for `org.maplibre_rs.MapLibreSdk` and `org.maplibre_rs.NativeMap`.

#![allow(unsafe_code)]

use jni::{
    objects::{JByteArray, JClass, JDoubleArray, JString},
    sys::{jboolean, jdouble, jint, jlong},
    EnvUnowned,
};

use crate::jni_helpers::{
    jni_create, jni_destroy, jni_get_camera, jni_needs_redraw, jni_render_rgba8, jni_resize,
    jni_set_camera, jni_set_feature_state_json, jni_set_geojson_source,
    jni_set_paint_property_json, jni_set_style_json,
};

macro_rules! export_jni_map_class {
    (
        $create:ident,
        $destroy:ident,
        $resize:ident,
        $set_camera:ident,
        $get_camera:ident,
        $set_style:ident,
        $set_paint:ident,
        $set_geojson:ident,
        $set_feature_state:ident,
        $needs_redraw:ident,
        $render_rgba8:ident
    ) => {
        /// Creates a `NativeMap` instance and returns its non-zero native handle.
        #[unsafe(no_mangle)]
        pub extern "system" fn $create<'caller>(
            env: EnvUnowned<'caller>,
            _class: JClass<'caller>,
            width: jint,
            height: jint,
            pixel_ratio: jdouble,
            cache_dir: JString<'caller>,
            max_pitch_deg: jdouble,
            style_json: JString<'caller>,
        ) -> jlong {
            jni_create(
                env,
                width,
                height,
                pixel_ratio,
                cache_dir,
                max_pitch_deg,
                style_json,
            )
        }

        /// Destroys a `NativeMap` handle created by `nativeCreate`.
        ///
        /// # Safety
        ///
        /// `map_ptr` must be `0` or a live handle returned by `nativeCreate`.
        #[unsafe(no_mangle)]
        pub unsafe extern "system" fn $destroy<'caller>(
            env: EnvUnowned<'caller>,
            _class: JClass<'caller>,
            map_ptr: jlong,
        ) {
            // SAFETY: Forwarded from JNI caller upholding handle ownership.
            unsafe { jni_destroy(env, map_ptr) };
        }

        /// Resizes the offscreen map target and updates `pixelRatio`.
        ///
        /// # Safety
        ///
        /// `map_ptr` must be a live handle returned by `nativeCreate`.
        #[unsafe(no_mangle)]
        pub unsafe extern "system" fn $resize<'caller>(
            env: EnvUnowned<'caller>,
            _class: JClass<'caller>,
            map_ptr: jlong,
            width: jint,
            height: jint,
            pixel_ratio: jdouble,
        ) -> jboolean {
            // SAFETY: Forwarded from JNI caller upholding handle validity.
            unsafe { jni_resize(env, map_ptr, width, height, pixel_ratio) }
        }

        /// Jumps the camera to `(latitude, longitude, zoom, bearingDeg, pitchDeg)`.
        ///
        /// # Safety
        ///
        /// `map_ptr` must be a live handle returned by `nativeCreate`.
        #[unsafe(no_mangle)]
        pub unsafe extern "system" fn $set_camera<'caller>(
            env: EnvUnowned<'caller>,
            _class: JClass<'caller>,
            map_ptr: jlong,
            latitude: jdouble,
            longitude: jdouble,
            zoom: jdouble,
            bearing_deg: jdouble,
            pitch_deg: jdouble,
        ) -> jboolean {
            // SAFETY: Forwarded from JNI caller upholding handle validity.
            unsafe {
                jni_set_camera(
                    env,
                    map_ptr,
                    latitude,
                    longitude,
                    zoom,
                    bearing_deg,
                    pitch_deg,
                )
            }
        }

        /// Returns `[latitude, longitude, zoom, bearingDeg, pitchDeg]` as a `double[5]`.
        ///
        /// # Safety
        ///
        /// `map_ptr` must be a live handle returned by `nativeCreate`.
        #[unsafe(no_mangle)]
        pub unsafe extern "system" fn $get_camera<'caller>(
            env: EnvUnowned<'caller>,
            _class: JClass<'caller>,
            map_ptr: jlong,
        ) -> JDoubleArray<'caller> {
            // SAFETY: Forwarded from JNI caller upholding handle validity.
            unsafe { jni_get_camera(env, map_ptr) }
        }

        /// Replaces the style JSON document while retaining camera state.
        ///
        /// # Safety
        ///
        /// `map_ptr` must be a live handle returned by `nativeCreate`.
        #[unsafe(no_mangle)]
        pub unsafe extern "system" fn $set_style<'caller>(
            env: EnvUnowned<'caller>,
            _class: JClass<'caller>,
            map_ptr: jlong,
            style_json: JString<'caller>,
        ) -> jboolean {
            // SAFETY: Forwarded from JNI caller upholding handle validity.
            unsafe { jni_set_style_json(env, map_ptr, style_json) }
        }

        /// Updates a layer paint property from JSON (`"null"` restores default).
        ///
        /// # Safety
        ///
        /// `map_ptr` must be a live handle returned by `nativeCreate`.
        #[unsafe(no_mangle)]
        pub unsafe extern "system" fn $set_paint<'caller>(
            env: EnvUnowned<'caller>,
            _class: JClass<'caller>,
            map_ptr: jlong,
            layer_id: JString<'caller>,
            property_name: JString<'caller>,
            value_json: JString<'caller>,
        ) -> jboolean {
            // SAFETY: Forwarded from JNI caller upholding handle validity.
            unsafe {
                jni_set_paint_property_json(env, map_ptr, layer_id, property_name, value_json)
            }
        }

        /// Replaces inline GeoJSON source data on `sourceId`.
        ///
        /// # Safety
        ///
        /// `map_ptr` must be a live handle returned by `nativeCreate`.
        #[unsafe(no_mangle)]
        pub unsafe extern "system" fn $set_geojson<'caller>(
            env: EnvUnowned<'caller>,
            _class: JClass<'caller>,
            map_ptr: jlong,
            source_id: JString<'caller>,
            geojson_json: JString<'caller>,
        ) -> jboolean {
            // SAFETY: Forwarded from JNI caller upholding handle validity.
            unsafe { jni_set_geojson_source(env, map_ptr, source_id, geojson_json) }
        }

        /// Merges JSON object `stateJson` into `featureId`'s runtime `feature-state`.
        ///
        /// # Safety
        ///
        /// `map_ptr` must be a live handle returned by `nativeCreate`.
        #[unsafe(no_mangle)]
        pub unsafe extern "system" fn $set_feature_state<'caller>(
            env: EnvUnowned<'caller>,
            _class: JClass<'caller>,
            map_ptr: jlong,
            source: JString<'caller>,
            source_layer: JString<'caller>,
            feature_id: JString<'caller>,
            state_json: JString<'caller>,
        ) -> jboolean {
            // SAFETY: Forwarded from JNI caller upholding handle validity.
            unsafe {
                jni_set_feature_state_json(
                    env,
                    map_ptr,
                    source,
                    source_layer,
                    feature_id,
                    state_json,
                )
            }
        }

        /// Returns `true` when dirty state, camera motion, or tile work requires another frame.
        ///
        /// # Safety
        ///
        /// `map_ptr` must be a live handle returned by `nativeCreate`.
        #[unsafe(no_mangle)]
        pub unsafe extern "system" fn $needs_redraw<'caller>(
            env: EnvUnowned<'caller>,
            _class: JClass<'caller>,
            map_ptr: jlong,
        ) -> jboolean {
            // SAFETY: Forwarded from JNI caller upholding handle validity.
            unsafe { jni_needs_redraw(env, map_ptr) }
        }

        /// Renders an RGBA8 frame into `outRgba` (`byte[width * height * 4]`) for `Bitmap`/`Surface`.
        ///
        /// # Safety
        ///
        /// `map_ptr` must be a live handle returned by `nativeCreate`.
        #[unsafe(no_mangle)]
        pub unsafe extern "system" fn $render_rgba8<'caller>(
            env: EnvUnowned<'caller>,
            _class: JClass<'caller>,
            map_ptr: jlong,
            out_rgba: JByteArray<'caller>,
        ) -> jboolean {
            // SAFETY: Forwarded from JNI caller upholding handle validity.
            unsafe { jni_render_rgba8(env, map_ptr, out_rgba) }
        }
    };
}

export_jni_map_class!(
    Java_org_maplibre_1rs_MapLibreSdk_nativeCreate,
    Java_org_maplibre_1rs_MapLibreSdk_nativeDestroy,
    Java_org_maplibre_1rs_MapLibreSdk_nativeResize,
    Java_org_maplibre_1rs_MapLibreSdk_nativeSetCamera,
    Java_org_maplibre_1rs_MapLibreSdk_nativeGetCamera,
    Java_org_maplibre_1rs_MapLibreSdk_nativeSetStyleJson,
    Java_org_maplibre_1rs_MapLibreSdk_nativeSetPaintPropertyJson,
    Java_org_maplibre_1rs_MapLibreSdk_nativeSetGeoJsonSource,
    Java_org_maplibre_1rs_MapLibreSdk_nativeSetFeatureStateJson,
    Java_org_maplibre_1rs_MapLibreSdk_nativeNeedsRedraw,
    Java_org_maplibre_1rs_MapLibreSdk_nativeRenderRgba8
);

export_jni_map_class!(
    Java_org_maplibre_1rs_NativeMap_nativeCreate,
    Java_org_maplibre_1rs_NativeMap_nativeDestroy,
    Java_org_maplibre_1rs_NativeMap_nativeResize,
    Java_org_maplibre_1rs_NativeMap_nativeSetCamera,
    Java_org_maplibre_1rs_NativeMap_nativeGetCamera,
    Java_org_maplibre_1rs_NativeMap_nativeSetStyleJson,
    Java_org_maplibre_1rs_NativeMap_nativeSetPaintPropertyJson,
    Java_org_maplibre_1rs_NativeMap_nativeSetGeoJsonSource,
    Java_org_maplibre_1rs_NativeMap_nativeSetFeatureStateJson,
    Java_org_maplibre_1rs_NativeMap_nativeNeedsRedraw,
    Java_org_maplibre_1rs_NativeMap_nativeRenderRgba8
);
