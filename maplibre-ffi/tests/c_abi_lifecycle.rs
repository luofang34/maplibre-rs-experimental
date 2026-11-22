#![allow(unsafe_code, clippy::expect_used, clippy::unwrap_used)]

use std::{
    ffi::{c_void, CStr, CString},
    ptr,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
};

use maplibre_ffi::{
    maplibre_last_error_message, maplibre_map_add_image_rgba8, maplibre_map_create,
    maplibre_map_destroy, maplibre_map_register_custom_layer, maplibre_map_remove_custom_layer,
    maplibre_map_render_rgba8, maplibre_map_set_style_json, MaplibreConfig,
    MaplibreCustomLayerCallbacks, MaplibreMap,
};

fn cstr(s: &str) -> CString {
    CString::new(s).expect("c string without interior nul")
}

fn last_error() -> String {
    unsafe { CStr::from_ptr(maplibre_last_error_message()) }
        .to_string_lossy()
        .into_owned()
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
    let map = unsafe { maplibre_map_create(&cfg, style.as_ptr()) };
    assert!(
        !map.is_null(),
        "maplibre_map_create failed: {}",
        last_error()
    );
    map
}

#[derive(Default)]
struct CallbackCounters {
    init: AtomicUsize,
    prepare: AtomicUsize,
    render: AtomicUsize,
    deinit: AtomicUsize,
    last_format: AtomicU64,
    last_samples: AtomicU64,
    last_width: AtomicU64,
    last_height: AtomicU64,
    last_zoom_bits: AtomicU64,
}

unsafe extern "C" fn on_init(user_data: *mut c_void, color_format: u32, sample_count: u32) {
    let counters = unsafe { &*(user_data as *const CallbackCounters) };
    counters.init.fetch_add(1, Ordering::SeqCst);
    counters
        .last_format
        .store(u64::from(color_format), Ordering::SeqCst);
    counters
        .last_samples
        .store(u64::from(sample_count), Ordering::SeqCst);
}

unsafe extern "C" fn on_prepare(user_data: *mut c_void, width: u32, height: u32, zoom: f64) {
    let counters = unsafe { &*(user_data as *const CallbackCounters) };
    counters.prepare.fetch_add(1, Ordering::SeqCst);
    counters
        .last_width
        .store(u64::from(width), Ordering::SeqCst);
    counters
        .last_height
        .store(u64::from(height), Ordering::SeqCst);
    counters
        .last_zoom_bits
        .store(zoom.to_bits(), Ordering::SeqCst);
}

unsafe extern "C" fn on_render(user_data: *mut c_void, _width: u32, _height: u32, _zoom: f64) {
    let counters = unsafe { &*(user_data as *const CallbackCounters) };
    counters.render.fetch_add(1, Ordering::SeqCst);
}

unsafe extern "C" fn on_deinit(user_data: *mut c_void) {
    let counters = unsafe { &*(user_data as *const CallbackCounters) };
    counters.deinit.fetch_add(1, Ordering::SeqCst);
}

fn callbacks_for(counters: &CallbackCounters) -> MaplibreCustomLayerCallbacks {
    MaplibreCustomLayerCallbacks {
        user_data: (counters as *const CallbackCounters) as *mut c_void,
        initialize: Some(on_init),
        prepare: Some(on_prepare),
        render: Some(on_render),
        deinitialize: Some(on_deinit),
    }
}

#[test]
fn add_image_and_render_validate_sizes_lengths_and_pointers_before_slice_creation() {
    let map = create_test_map(16, 16, r#"{"version":8,"sources":{},"layers":[]}"#);
    let id = cstr("marker");
    let valid_px = [255_u8; 4];

    assert!(!unsafe {
        maplibre_map_add_image_rgba8(map, id.as_ptr(), 0, 1, valid_px.as_ptr(), 0, 1.0, false)
    });
    assert!(last_error().contains("invalid dimensions"));

    assert!(!unsafe {
        maplibre_map_add_image_rgba8(map, id.as_ptr(), 1, 0, valid_px.as_ptr(), 0, 1.0, false)
    });
    assert!(last_error().contains("invalid dimensions"));

    // Multiplication overflow and exceeding `isize::MAX` must fail cleanly before `from_raw_parts`.
    assert!(!unsafe {
        maplibre_map_add_image_rgba8(
            map,
            id.as_ptr(),
            u32::MAX,
            u32::MAX,
            valid_px.as_ptr(),
            usize::MAX,
            1.0,
            false,
        )
    });
    assert!(last_error().contains("invalid dimensions"));

    // Wrong `rgba_len` (even `usize::MAX` with non-null small buffer) must fail before `from_raw_parts`.
    assert!(!unsafe {
        maplibre_map_add_image_rgba8(
            map,
            id.as_ptr(),
            1,
            1,
            valid_px.as_ptr(),
            usize::MAX,
            1.0,
            false,
        )
    });
    assert!(last_error().contains("requires 4 bytes"));

    assert!(!unsafe {
        maplibre_map_add_image_rgba8(map, id.as_ptr(), 1, 1, ptr::null(), 4, 1.0, false)
    });
    assert!(last_error().contains("rgba"));

    assert!(!unsafe {
        maplibre_map_add_image_rgba8(
            map,
            id.as_ptr(),
            1,
            1,
            valid_px.as_ptr(),
            4,
            f32::NAN,
            false,
        )
    });
    assert!(last_error().contains("pixel_ratio"));

    assert!(!unsafe { maplibre_map_render_rgba8(map, ptr::null_mut(), 16 * 16 * 4) });
    assert!(last_error().contains("out_rgba"));

    let mut small = [0_u8; 8];
    assert!(!unsafe { maplibre_map_render_rgba8(map, small.as_mut_ptr(), small.len()) });
    assert!(last_error().contains("requires"));

    unsafe { maplibre_map_destroy(map) };
}

#[test]
fn custom_layer_c_abi_lifecycle_runs_init_prepare_render_and_deinit() {
    let style_with = r##"{
        "version": 8,
        "center": [0.0, 0.0],
        "zoom": 3.5,
        "sources": {},
        "layers": [
            { "id": "bg", "type": "background", "paint": { "background-color": "#112233" } },
            { "id": "overlay", "type": "custom" }
        ]
    }"##;
    let style_without = r##"{
        "version": 8,
        "center": [0.0, 0.0],
        "zoom": 3.5,
        "sources": {},
        "layers": [
            { "id": "bg", "type": "background", "paint": { "background-color": "#112233" } }
        ]
    }"##;
    let map = create_test_map(32, 32, style_with);
    let layer_id = cstr("overlay");
    let empty_id = cstr("");

    // Null / empty argument validation.
    let dummy = CallbackCounters::default();
    let dummy_cb = callbacks_for(&dummy);
    assert!(!unsafe {
        maplibre_map_register_custom_layer(ptr::null_mut(), layer_id.as_ptr(), &dummy_cb)
    });
    assert!(!unsafe { maplibre_map_register_custom_layer(map, ptr::null(), &dummy_cb) });
    assert!(!unsafe { maplibre_map_register_custom_layer(map, empty_id.as_ptr(), &dummy_cb) });
    assert!(!unsafe { maplibre_map_register_custom_layer(map, layer_id.as_ptr(), ptr::null()) });
    assert!(!unsafe { maplibre_map_remove_custom_layer(ptr::null_mut(), layer_id.as_ptr()) });
    assert!(!unsafe { maplibre_map_remove_custom_layer(map, empty_id.as_ptr()) });

    let c1 = CallbackCounters::default();
    let cb1 = callbacks_for(&c1);
    assert!(unsafe { maplibre_map_register_custom_layer(map, layer_id.as_ptr(), &cb1) });
    assert_eq!(last_error(), "");

    let mut pixels = vec![0_u8; 32 * 32 * 4];
    assert!(unsafe { maplibre_map_render_rgba8(map, pixels.as_mut_ptr(), pixels.len()) });
    assert_eq!(c1.init.load(Ordering::SeqCst), 1);
    assert!(c1.prepare.load(Ordering::SeqCst) >= 1);
    assert!(c1.render.load(Ordering::SeqCst) >= 1);
    assert_eq!(c1.deinit.load(Ordering::SeqCst), 0);
    assert_ne!(c1.last_format.load(Ordering::SeqCst), 0);
    assert!(c1.last_samples.load(Ordering::SeqCst) >= 1);
    assert_eq!(c1.last_width.load(Ordering::SeqCst), 32);
    assert_eq!(c1.last_height.load(Ordering::SeqCst), 32);
    assert!((f64::from_bits(c1.last_zoom_bits.load(Ordering::SeqCst)) - 3.5).abs() < 1e-6);

    // Style replacement removing the custom layer triggers immediate deinitialize.
    let no_custom = cstr(style_without);
    assert!(unsafe { maplibre_map_set_style_json(map, no_custom.as_ptr()) });
    assert_eq!(c1.deinit.load(Ordering::SeqCst), 1);

    // Re-adding the layer in style re-initializes the still-registered layer.
    let with_custom = cstr(style_with);
    assert!(unsafe { maplibre_map_set_style_json(map, with_custom.as_ptr()) });
    assert!(unsafe { maplibre_map_render_rgba8(map, pixels.as_mut_ptr(), pixels.len()) });
    assert_eq!(c1.init.load(Ordering::SeqCst), 2);
    assert_eq!(c1.deinit.load(Ordering::SeqCst), 1);

    // Replacing registered callbacks deinitializes old callbacks immediately.
    let c2 = CallbackCounters::default();
    let cb2 = callbacks_for(&c2);
    assert!(unsafe { maplibre_map_register_custom_layer(map, layer_id.as_ptr(), &cb2) });
    assert_eq!(c1.deinit.load(Ordering::SeqCst), 2);
    assert!(unsafe { maplibre_map_render_rgba8(map, pixels.as_mut_ptr(), pixels.len()) });
    assert_eq!(c2.init.load(Ordering::SeqCst), 1);

    // Explicit remove returns true and deinitializes; second remove returns false cleanly.
    assert!(unsafe { maplibre_map_remove_custom_layer(map, layer_id.as_ptr()) });
    assert_eq!(last_error(), "");
    assert_eq!(c2.deinit.load(Ordering::SeqCst), 1);
    assert!(!unsafe { maplibre_map_remove_custom_layer(map, layer_id.as_ptr()) });
    assert_eq!(last_error(), "");

    // Re-register + initialize, then destroy map -> deinitializes on map destruction.
    let c3 = CallbackCounters::default();
    let cb3 = callbacks_for(&c3);
    assert!(unsafe { maplibre_map_register_custom_layer(map, layer_id.as_ptr(), &cb3) });
    assert!(unsafe { maplibre_map_render_rgba8(map, pixels.as_mut_ptr(), pixels.len()) });
    assert_eq!(c3.init.load(Ordering::SeqCst), 1);
    assert_eq!(c3.deinit.load(Ordering::SeqCst), 0);

    unsafe { maplibre_map_destroy(map) };
    assert_eq!(c1.deinit.load(Ordering::SeqCst), 2);
    assert_eq!(c2.deinit.load(Ordering::SeqCst), 1);
    assert_eq!(c3.deinit.load(Ordering::SeqCst), 1);
}
