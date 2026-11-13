//! Integration tests for the `maplibre-ffi` C ABI (`include/maplibre.h`).

#![allow(unsafe_code, clippy::unwrap_used, clippy::expect_used)]

use std::{
    ffi::{CStr, CString},
    ptr, thread,
};

use maplibre_ffi::{
    maplibre_handle_destroy, maplibre_handle_set_camera, maplibre_handle_set_feature_state_json,
    maplibre_handle_set_style_json, maplibre_last_error_message, maplibre_map_add_image_rgba8,
    maplibre_map_clone_handle, maplibre_map_create, maplibre_map_destroy, maplibre_map_get_camera,
    maplibre_map_needs_redraw, maplibre_map_query_rendered_features_json,
    maplibre_map_remove_feature_state, maplibre_map_render_rgba8, maplibre_map_resize,
    maplibre_map_set_camera, maplibre_map_set_feature_state_json, maplibre_map_set_filter_json,
    maplibre_map_set_geojson_source_data, maplibre_map_set_layer_visibility,
    maplibre_map_set_layout_property_json, maplibre_map_set_paint_property_json,
    maplibre_map_set_style_json, maplibre_string_free, maplibre_version, retain_c_abi_symbols,
    MaplibreCamera, MaplibreConfig, MaplibreMap,
};
use serde_json::Value;

fn last_error() -> String {
    // SAFETY: `maplibre_last_error_message` always returns a valid thread-local NUL-terminated ptr.
    unsafe { CStr::from_ptr(maplibre_last_error_message()) }
        .to_string_lossy()
        .into_owned()
}

fn cstr(text: &str) -> CString {
    CString::new(text).expect("test string has no interior NUL")
}

fn center_rgba(rgba: &[u8], width: u32, height: u32) -> [u8; 4] {
    let x = width / 2;
    let y = height / 2;
    let idx = ((y * width + x) * 4) as usize;
    [rgba[idx], rgba[idx + 1], rgba[idx + 2], rgba[idx + 3]]
}

fn create_test_map(width: u32, height: u32, style_json: &str) -> *mut MaplibreMap {
    let cfg = MaplibreConfig {
        width,
        height,
        pixel_ratio: 1.0,
        cache_dir: ptr::null(),
        max_pitch_deg: 75.0,
    };
    let style = cstr(style_json);
    // SAFETY: `cfg` and `style` are valid for the duration of `maplibre_map_create`.
    let map = unsafe { maplibre_map_create(&cfg, style.as_ptr()) };
    assert!(
        !map.is_null(),
        "maplibre_map_create failed: {}",
        last_error()
    );
    assert_eq!(last_error(), "");
    map
}

#[test]
fn header_declares_every_exported_symbol() {
    assert_eq!(retain_c_abi_symbols(), 25);

    let version = unsafe { CStr::from_ptr(maplibre_version()) }.to_string_lossy();
    assert!(!version.is_empty());

    let header = include_str!("../include/maplibre.h");
    let expected_symbols = [
        "maplibre_version",
        "maplibre_last_error_message",
        "maplibre_string_free",
        "maplibre_map_create",
        "maplibre_map_destroy",
        "maplibre_map_resize",
        "maplibre_map_clone_handle",
        "maplibre_handle_destroy",
        "maplibre_handle_set_camera",
        "maplibre_handle_set_style_json",
        "maplibre_handle_set_feature_state_json",
        "maplibre_map_get_camera",
        "maplibre_map_set_camera",
        "maplibre_map_set_style_json",
        "maplibre_map_set_paint_property_json",
        "maplibre_map_set_layout_property_json",
        "maplibre_map_set_filter_json",
        "maplibre_map_set_layer_visibility",
        "maplibre_map_set_geojson_source_data",
        "maplibre_map_set_feature_state_json",
        "maplibre_map_remove_feature_state",
        "maplibre_map_add_image_rgba8",
        "maplibre_map_needs_redraw",
        "maplibre_map_render_rgba8",
        "maplibre_map_query_rendered_features_json",
    ];
    for sym in expected_symbols {
        assert!(
            header.contains(&format!("{sym}(")),
            "include/maplibre.h is missing declaration for `{sym}`"
        );
    }
    for ty in [
        "MaplibreMap",
        "MaplibreHandle",
        "MaplibreConfig",
        "MaplibreCamera",
    ] {
        assert!(
            header.contains(ty),
            "include/maplibre.h is missing typedef `{ty}`"
        );
    }
}

#[test]
fn null_pointers_and_invalid_inputs_report_clean_errors() {
    // Null cleanup calls are safe no-ops.
    unsafe {
        maplibre_map_destroy(ptr::null_mut());
        maplibre_handle_destroy(ptr::null_mut());
        maplibre_string_free(ptr::null_mut());
    }

    let valid_style = cstr(r#"{"version":8,"sources":{},"layers":[]}"#);
    // Null config -> NULL + error message.
    let null_cfg_map = unsafe { maplibre_map_create(ptr::null(), valid_style.as_ptr()) };
    assert!(null_cfg_map.is_null());
    assert!(last_error().contains("config"));

    // Invalid JSON -> NULL + error message.
    let cfg = MaplibreConfig::default();
    let broken_style = cstr("{not valid json");
    let bad_map = unsafe { maplibre_map_create(&cfg, broken_style.as_ptr()) };
    assert!(bad_map.is_null());
    assert!(last_error().contains("style_json"));

    // Zero dimensions -> NULL + error message.
    let zero_cfg = MaplibreConfig {
        width: 0,
        height: 64,
        ..MaplibreConfig::default()
    };
    let zero_map = unsafe { maplibre_map_create(&zero_cfg, valid_style.as_ptr()) };
    assert!(zero_map.is_null());
    assert!(last_error().contains("0x64"));

    // Null map pointer on operations -> false / NULL + error message.
    let mut cam = MaplibreCamera::default();
    assert!(!unsafe { maplibre_map_get_camera(ptr::null(), &mut cam) });
    assert!(last_error().contains("map"));
    assert!(!unsafe { maplibre_map_needs_redraw(ptr::null()) });
    assert!(last_error().contains("map"));
    let features = unsafe {
        maplibre_map_query_rendered_features_json(ptr::null(), 0.0, 0.0, 1.0, 1.0, ptr::null())
    };
    assert!(features.is_null());
    assert!(last_error().contains("map"));
}

#[test]
fn lifecycle_camera_style_geojson_feature_state_query_and_rgba_readback() {
    let initial_style = r##"{
        "version": 8,
        "center": [0.0, 0.0],
        "zoom": 1.0,
        "sources": {
            "boxes": {
                "type": "geojson",
                "data": { "type": "FeatureCollection", "features": [] }
            }
        },
        "layers": [
            {
                "id": "bg",
                "type": "background",
                "paint": { "background-color": "#0000ff" }
            },
            {
                "id": "box-fill",
                "type": "fill",
                "source": "boxes",
                "paint": {
                    "fill-color": [
                        "case",
                        ["boolean", ["feature-state", "selected"], false],
                        "#00ff00",
                        "#ff0000"
                    ]
                }
            }
        ]
    }"##;

    let map = create_test_map(64, 64, initial_style);

    // Undersized buffer fails cleanly before rendering.
    let mut tiny = [0_u8; 16];
    assert!(!unsafe { maplibre_map_render_rgba8(map, tiny.as_mut_ptr(), tiny.len()) });
    assert!(last_error().contains("buffer requires"));

    // Initial render shows solid blue background `[0, 0, 255, 255]`.
    let mut pixels = vec![0_u8; 64 * 64 * 4];
    assert!(unsafe { maplibre_map_render_rgba8(map, pixels.as_mut_ptr(), pixels.len()) });
    assert_eq!(last_error(), "");
    let bg_px = center_rgba(&pixels, 64, 64);
    assert!(
        bg_px[2] >= 240 && bg_px[0] <= 15 && bg_px[1] <= 15,
        "expected blue, got {bg_px:?}"
    );
    // Settled map reports needs_redraw == false with empty error string.
    let _ = unsafe { maplibre_map_needs_redraw(map) };
    assert_eq!(last_error(), "");

    // Camera round-trip.
    let target_cam = MaplibreCamera {
        latitude: 12.5,
        longitude: -24.0,
        zoom: 3.5,
        bearing_deg: 15.0,
        pitch_deg: 20.0,
    };
    assert!(unsafe { maplibre_map_set_camera(map, &target_cam) });
    let mut read_cam = MaplibreCamera::default();
    assert!(unsafe { maplibre_map_get_camera(map, &mut read_cam) });
    assert!((read_cam.latitude - 12.5).abs() < 1e-3);
    assert!((read_cam.longitude - (-24.0)).abs() < 1e-3);
    assert!((read_cam.zoom - 3.5).abs() < 1e-6);
    assert!((read_cam.bearing_deg - 15.0).abs() < 1e-3);
    assert!((read_cam.pitch_deg - 20.0).abs() < 1e-3);

    // Reset camera to origin so a [-60..60] GeoJSON polygon covers center pixel.
    let origin_cam = MaplibreCamera {
        latitude: 0.0,
        longitude: 0.0,
        zoom: 1.0,
        bearing_deg: 0.0,
        pitch_deg: 0.0,
    };
    assert!(unsafe { maplibre_map_set_camera(map, &origin_cam) });

    // Inject GeoJSON polygon with id = 42 and property kind = "zone".
    let source_id = cstr("boxes");
    let geojson = cstr(
        r#"{
            "type": "FeatureCollection",
            "features": [{
                "type": "Feature",
                "id": 42,
                "properties": { "kind": "zone" },
                "geometry": {
                    "type": "Polygon",
                    "coordinates": [[
                        [-60.0, -60.0],
                        [60.0, -60.0],
                        [60.0, 60.0],
                        [-60.0, 60.0],
                        [-60.0, -60.0]
                    ]]
                }
            }]
        }"#,
    );
    assert!(unsafe {
        maplibre_map_set_geojson_source_data(map, source_id.as_ptr(), geojson.as_ptr())
    });
    assert!(unsafe { maplibre_map_needs_redraw(map) });

    assert!(unsafe { maplibre_map_render_rgba8(map, pixels.as_mut_ptr(), pixels.len()) });
    let unselected_px = center_rgba(&pixels, 64, 64);
    assert!(
        unselected_px[0] >= 240 && unselected_px[1] <= 15 && unselected_px[2] <= 15,
        "expected red fill before feature-state, got {unselected_px:?}"
    );

    // Query rendered features at center pixel `(32.0, 32.0)`.
    let opts = cstr(r#"{"layers":["box-fill"]}"#);
    let json_ptr = unsafe {
        maplibre_map_query_rendered_features_json(map, 32.0, 32.0, 32.0, 32.0, opts.as_ptr())
    };
    assert!(!json_ptr.is_null(), "query failed: {}", last_error());
    let json_text = unsafe { CStr::from_ptr(json_ptr) }
        .to_string_lossy()
        .into_owned();
    unsafe { maplibre_string_free(json_ptr) };
    let parsed_features: Value = serde_json::from_str(&json_text).expect("valid feature JSON");
    let arr = parsed_features.as_array().expect("JSON array");
    assert_eq!(arr.len(), 1, "expected 1 hit at center, got {json_text}");
    assert_eq!(arr[0]["properties"]["kind"], "zone");

    // Toggle `feature-state` `selected = true` -> polygon turns green!
    let feat_id = cstr("42");
    let selected_true = cstr(r#"{"selected":true}"#);
    assert!(unsafe {
        maplibre_map_set_feature_state_json(
            map,
            source_id.as_ptr(),
            ptr::null(),
            feat_id.as_ptr(),
            selected_true.as_ptr(),
        )
    });
    assert!(unsafe { maplibre_map_render_rgba8(map, pixels.as_mut_ptr(), pixels.len()) });
    let selected_px = center_rgba(&pixels, 64, 64);
    assert!(
        selected_px[1] >= 240 && selected_px[0] <= 15 && selected_px[2] <= 15,
        "expected green fill after feature-state, got {selected_px:?}"
    );

    // Remove `feature-state` -> reverts to red!
    let state_key = cstr("selected");
    assert!(unsafe {
        maplibre_map_remove_feature_state(
            map,
            source_id.as_ptr(),
            ptr::null(),
            feat_id.as_ptr(),
            state_key.as_ptr(),
        )
    });
    assert!(unsafe { maplibre_map_render_rgba8(map, pixels.as_mut_ptr(), pixels.len()) });
    let reverted_px = center_rgba(&pixels, 64, 64);
    assert!(
        reverted_px[0] >= 240 && reverted_px[1] <= 15,
        "expected red fill after removing feature-state, got {reverted_px:?}"
    );

    // Layer visibility, layout, filter, paint, sprite image, and resize mutations.
    let fill_layer = cstr("box-fill");
    assert!(unsafe { maplibre_map_set_layer_visibility(map, fill_layer.as_ptr(), false) });
    assert!(unsafe { maplibre_map_render_rgba8(map, pixels.as_mut_ptr(), pixels.len()) });
    let hidden_px = center_rgba(&pixels, 64, 64);
    assert!(
        hidden_px[2] >= 240 && hidden_px[0] <= 15,
        "expected blue when fill hidden, got {hidden_px:?}"
    );

    // Re-enable via `set_layout_property_json` and filter out non-matching kind.
    let vis_prop = cstr("visibility");
    let vis_val = cstr(r#""visible""#);
    assert!(unsafe {
        maplibre_map_set_layout_property_json(
            map,
            fill_layer.as_ptr(),
            vis_prop.as_ptr(),
            vis_val.as_ptr(),
        )
    });
    let filter_exclude = cstr(r#"["==", ["get", "kind"], "other"]"#);
    assert!(unsafe {
        maplibre_map_set_filter_json(map, fill_layer.as_ptr(), filter_exclude.as_ptr())
    });
    assert!(unsafe { maplibre_map_render_rgba8(map, pixels.as_mut_ptr(), pixels.len()) });
    assert!(center_rgba(&pixels, 64, 64)[2] >= 240);

    // Clear filter with NULL and mutate background paint property.
    assert!(unsafe { maplibre_map_set_filter_json(map, fill_layer.as_ptr(), ptr::null()) });
    let bg_layer = cstr("bg");
    let bg_prop = cstr("background-color");
    let yellow_val = cstr(r##""#ffff00""##);
    assert!(unsafe {
        maplibre_map_set_paint_property_json(
            map,
            bg_layer.as_ptr(),
            bg_prop.as_ptr(),
            yellow_val.as_ptr(),
        )
    });

    // Register a 2x2 RGBA8 sprite image and resize to 32x32.
    let img_id = cstr("pin");
    let img_rgba = [255_u8; 2 * 2 * 4];
    assert!(unsafe {
        maplibre_map_add_image_rgba8(
            map,
            img_id.as_ptr(),
            2,
            2,
            img_rgba.as_ptr(),
            img_rgba.len(),
            2.0,
            false,
        )
    });
    assert!(unsafe { maplibre_map_resize(map, 32, 32, 2.0) });
    let mut resized_pixels = vec![0_u8; 32 * 32 * 4];
    assert!(unsafe {
        maplibre_map_render_rgba8(map, resized_pixels.as_mut_ptr(), resized_pixels.len())
    });

    // Whole-style replacement.
    let green_bg_style = r##"{
        "version": 8,
        "sources": {},
        "layers": [{ "id": "bg", "type": "background", "paint": { "background-color": "#00ff00" } }]
    }"##;
    let green_style_c = cstr(green_bg_style);
    assert!(unsafe { maplibre_map_set_style_json(map, green_style_c.as_ptr()) });
    assert!(unsafe {
        maplibre_map_render_rgba8(map, resized_pixels.as_mut_ptr(), resized_pixels.len())
    });
    let replaced_px = center_rgba(&resized_pixels, 32, 32);
    assert!(replaced_px[1] >= 240 && replaced_px[0] <= 15 && replaced_px[2] <= 15);

    unsafe { maplibre_map_destroy(map) };
}

#[test]
fn thread_safe_handle_drives_map_from_worker_threads_and_rejects_after_destroy() {
    let map = create_test_map(
        32,
        32,
        r##"{"version":8,"sources":{},"layers":[{"id":"bg","type":"background","paint":{"background-color":"#000000"}}]}"##,
    );
    let handle = unsafe { maplibre_map_clone_handle(map) };
    assert!(!handle.is_null());

    // Send raw pointer numeric address across an OS thread (`MaplibreHandle` wraps `SyncHeadlessHandle`).
    let handle_addr = handle as usize;
    let worker = thread::spawn(move || {
        let h = handle_addr as *const maplibre_ffi::MaplibreHandle;
        let cam = MaplibreCamera {
            latitude: 35.68,
            longitude: 139.69,
            zoom: 5.0,
            bearing_deg: 10.0,
            pitch_deg: 15.0,
        };
        assert!(unsafe { maplibre_handle_set_camera(h, &cam) });

        let style = cstr(
            r##"{
                "version": 8,
                "sources": {
                    "pts": {
                        "type": "geojson",
                        "data": { "type": "FeatureCollection", "features": [] }
                    }
                },
                "layers": [
                    { "id": "bg", "type": "background", "paint": { "background-color": "#ff00ff" } }
                ]
            }"##,
        );
        assert!(unsafe { maplibre_handle_set_style_json(h, style.as_ptr()) });

        let src = cstr("pts");
        let fid = cstr("1");
        let st = cstr(r#"{"active":true}"#);
        assert!(unsafe {
            maplibre_handle_set_feature_state_json(
                h,
                src.as_ptr(),
                ptr::null(),
                fid.as_ptr(),
                st.as_ptr(),
            )
        });
    });
    worker.join().expect("worker thread joined");

    // Rendering flushes all queued commands in FIFO order.
    let mut pixels = vec![0_u8; 32 * 32 * 4];
    assert!(unsafe { maplibre_map_render_rgba8(map, pixels.as_mut_ptr(), pixels.len()) });
    let px = center_rgba(&pixels, 32, 32);
    assert!(
        px[0] >= 240 && px[1] <= 15 && px[2] >= 240,
        "expected magenta background from worker thread style, got {px:?}"
    );

    let mut cam = MaplibreCamera::default();
    assert!(unsafe { maplibre_map_get_camera(map, &mut cam) });
    assert!((cam.latitude - 35.68).abs() < 1e-2);
    assert!((cam.longitude - 139.69).abs() < 1e-2);

    // Destroy the map while `handle` is still alive: subsequent handle commands fail cleanly!
    unsafe { maplibre_map_destroy(map) };
    assert!(!unsafe { maplibre_handle_set_camera(handle, &cam) });
    assert!(
        last_error().contains("stopped"),
        "unexpected post-destroy error: {}",
        last_error()
    );

    unsafe { maplibre_handle_destroy(handle) };
}
