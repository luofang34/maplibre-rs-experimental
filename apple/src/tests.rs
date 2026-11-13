use std::{ffi::CString, ptr, thread};

use maplibre::projection::ProjectionType;

use crate::{
    build_apple_config_from_c, init_apple_logging, maplibre_apple_run_with_style_json,
    maplibre_handle_destroy, maplibre_handle_set_camera, maplibre_map_clone_handle,
    maplibre_map_create, maplibre_map_destroy, maplibre_map_get_camera, maplibre_map_render_rgba8,
    maplibre_map_set_feature_state_json, maplibre_map_set_geojson_source_data,
    retain_c_abi_symbols, AppleMapConfig, MaplibreCamera, MaplibreConfig,
};

#[test]
fn apple_config_and_c_entrypoint_validate_style_globe_and_pitch() {
    init_apple_logging();
    init_apple_logging(); // Idempotent!

    assert_eq!(retain_c_abi_symbols(), 25);

    let cfg = AppleMapConfig::default()
        .with_style_json(r#"{"version":8,"name":"SwiftMap","sources":{},"layers":[]}"#)
        .with_cache_dir("/tmp/maplibre-apple-cache")
        .with_globe(true)
        .with_max_pitch_deg(80.0);

    let style = cfg.resolve_style().unwrap();
    assert_eq!(style.name.as_deref(), Some("SwiftMap"));
    assert_eq!(
        style.projection.as_ref().map(|p| &p.projection_type),
        Some(&ProjectionType::Globe)
    );
    let headed = cfg.headed_options().unwrap();
    assert!((headed.max_pitch_degrees - 80.0).abs() < f64::EPSILON);

    let c_style = CString::new(r#"{"version":8,"sources":{},"layers":[]}"#).unwrap();
    let c_cache = CString::new("/tmp/apple-cache").unwrap();
    let parsed =
        unsafe { build_apple_config_from_c(c_style.as_ptr(), c_cache.as_ptr(), true, 72.0) }
            .unwrap();
    assert!(parsed.globe);
    assert_eq!(
        parsed.cache_dir.as_deref().and_then(|p| p.to_str()),
        Some("/tmp/apple-cache")
    );

    // Invalid JSON and NaN pitch fail cleanly in `maplibre_apple_run_with_style_json` before window creation!
    let invalid_style = CString::new("{not-json").unwrap();
    assert!(!unsafe {
        maplibre_apple_run_with_style_json(invalid_style.as_ptr(), ptr::null(), false, 60.0)
    });
    assert!(!unsafe {
        maplibre_apple_run_with_style_json(c_style.as_ptr(), ptr::null(), false, f64::NAN)
    });
}

#[test]
fn xcframework_reexports_complete_c_abi_and_renders_offscreen() {
    let style = CString::new(
        r##"{
            "version": 8,
            "center": [0.0, 0.0],
            "zoom": 1.0,
            "sources": {
                "items": {
                    "type": "geojson",
                    "data": { "type": "FeatureCollection", "features": [] }
                }
            },
            "layers": [
                { "id": "bg", "type": "background", "paint": { "background-color": "#0000ff" } },
                {
                    "id": "poly",
                    "type": "fill",
                    "source": "items",
                    "paint": {
                        "fill-color": [
                            "case",
                            ["boolean", ["feature-state", "lit"], false],
                            "#ffff00",
                            "#ff0000"
                        ]
                    }
                }
            ]
        }"##,
    )
    .unwrap();
    let cfg = MaplibreConfig {
        width: 32,
        height: 32,
        pixel_ratio: 2.0,
        cache_dir: ptr::null(),
        max_pitch_deg: 60.0,
    };
    let map = unsafe { maplibre_map_create(&cfg, style.as_ptr()) };
    assert!(!map.is_null());

    // Thread-safe handle from Swift GCD / Task thread.
    let handle = unsafe { maplibre_map_clone_handle(map) };
    assert!(!handle.is_null());
    let handle_addr = handle as usize;
    thread::spawn(move || {
        let h = handle_addr as *const crate::MaplibreHandle;
        let cam = MaplibreCamera {
            latitude: 0.0,
            longitude: 0.0,
            zoom: 1.0,
            bearing_deg: 0.0,
            pitch_deg: 0.0,
        };
        assert!(unsafe { maplibre_handle_set_camera(h, &cam) });
    })
    .join()
    .unwrap();

    let src = CString::new("items").unwrap();
    let geojson = CString::new(
        r#"{
            "type": "FeatureCollection",
            "features": [{
                "type": "Feature",
                "id": 9,
                "properties": {},
                "geometry": {
                    "type": "Polygon",
                    "coordinates": [[[-60,-60],[60,-60],[60,60],[-60,60],[-60,-60]]]
                }
            }]
        }"#,
    )
    .unwrap();
    let fid = CString::new("9").unwrap();
    let lit = CString::new(r#"{"lit":true}"#).unwrap();

    assert!(unsafe { maplibre_map_set_geojson_source_data(map, src.as_ptr(), geojson.as_ptr()) });
    assert!(unsafe {
        maplibre_map_set_feature_state_json(
            map,
            src.as_ptr(),
            ptr::null(),
            fid.as_ptr(),
            lit.as_ptr(),
        )
    });

    let mut rgba = vec![0_u8; 32 * 32 * 4];
    assert!(unsafe { maplibre_map_render_rgba8(map, rgba.as_mut_ptr(), rgba.len()) });
    let center = ((16 * 32 + 16) * 4) as usize;
    assert!(rgba[center] >= 240 && rgba[center + 1] >= 240 && rgba[center + 2] <= 15);

    let mut cam = MaplibreCamera::default();
    assert!(unsafe { maplibre_map_get_camera(map, &mut cam) });
    assert!((cam.zoom - 1.0).abs() < 1e-6);

    unsafe {
        maplibre_handle_destroy(handle);
        maplibre_map_destroy(map);
    }
}
