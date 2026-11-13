use std::thread;

use maplibre::{
    coords::LatLon,
    projection::ProjectionType,
    render::{settings::Backends, view_state::CameraOptions},
};

use crate::{AndroidMapOptions, AndroidSdkError, CameraState, MapEngineConfig, NativeMap};

#[test]
fn options_resolve_custom_style_globe_and_pitch() {
    let opts = AndroidMapOptions::default()
        .with_style_json(r#"{"version":8,"name":"Mobile","sources":{},"layers":[]}"#)
        .with_cache_dir("/data/user/0/org.maplibre_rs/cache")
        .with_backends(Backends::VULKAN)
        .with_max_pitch_deg(78.0)
        .with_globe(true);

    let style = opts.resolve_style().unwrap();
    assert_eq!(style.name.as_deref(), Some("Mobile"));
    assert_eq!(
        style.projection.as_ref().map(|p| &p.projection_type),
        Some(&ProjectionType::Globe)
    );

    let headed = opts.headed_options().unwrap();
    assert!((headed.max_pitch_degrees - 78.0).abs() < f64::EPSILON);
    assert_eq!(opts.wgpu_settings().backends, Some(Backends::VULKAN));

    let bad = AndroidMapOptions::default().with_style_json("{broken");
    assert!(matches!(bad.resolve_style(), Err(AndroidSdkError::Ffi(_))));

    let nan_pitch = AndroidMapOptions::default().with_max_pitch_deg(f64::NAN);
    assert!(nan_pitch.headed_options().is_err());
}

#[test]
fn native_map_renders_mutates_and_handles_multiple_instances_across_threads() {
    let style_a = r##"{
        "version": 8,
        "center": [0.0, 0.0],
        "zoom": 1.0,
        "sources": {
            "poly": {
                "type": "geojson",
                "data": { "type": "FeatureCollection", "features": [] }
            }
        },
        "layers": [
            { "id": "bg", "type": "background", "paint": { "background-color": "#ff0000" } },
            {
                "id": "fill",
                "type": "fill",
                "source": "poly",
                "paint": {
                    "fill-color": [
                        "case",
                        ["boolean", ["feature-state", "active"], false],
                        "#00ff00",
                        "#0000ff"
                    ]
                }
            }
        ]
    }"##;
    let style_b = r##"{
        "version": 8,
        "sources": {},
        "layers": [{ "id": "bg", "type": "background", "paint": { "background-color": "#00ffff" } }]
    }"##;

    let map_a = NativeMap::create(
        MapEngineConfig {
            width: 32,
            height: 32,
            pixel_ratio: 2.0,
            cache_dir: None,
            max_pitch_deg: 60.0,
        },
        style_a,
    )
    .unwrap();
    let map_b = NativeMap::create(
        MapEngineConfig {
            width: 16,
            height: 16,
            pixel_ratio: 1.0,
            cache_dir: None,
            max_pitch_deg: 60.0,
        },
        style_b,
    )
    .unwrap();

    // Transfer `map_a` through a JNI-style `jlong` handle and drive via `SyncHeadlessHandle`.
    let raw_a = map_a.into_raw_handle();
    assert_ne!(raw_a, 0);
    let borrowed_a = unsafe { NativeMap::borrow_raw(raw_a) }.unwrap();

    let handle_a = borrowed_a.clone_handle().unwrap();
    thread::spawn(move || {
        handle_a
            .call_blocking(|m| {
                m.jump_to(CameraOptions {
                    center: Some(LatLon::new(0.0, 0.0)),
                    zoom: Some(1.0),
                    ..CameraOptions::default()
                })
            })
            .unwrap()
            .unwrap();
    })
    .join()
    .unwrap();

    borrowed_a
        .set_geojson_source(
            "poly",
            r#"{
                "type": "FeatureCollection",
                "features": [{
                    "type": "Feature",
                    "id": 7,
                    "properties": {},
                    "geometry": {
                        "type": "Polygon",
                        "coordinates": [[[-60,-60],[60,-60],[60,60],[-60,60],[-60,-60]]]
                    }
                }]
            }"#,
        )
        .unwrap();
    borrowed_a
        .set_feature_state_json("poly", None, "7", r#"{"active":true}"#)
        .unwrap();
    assert!(borrowed_a.needs_redraw().unwrap());

    let mut rgba_a = vec![0_u8; 32 * 32 * 4];
    borrowed_a.render_rgba8_into(&mut rgba_a).unwrap();
    let center_a = (16 * 32 + 16) * 4;
    assert!(rgba_a[center_a + 1] >= 240 && rgba_a[center_a] <= 15);

    // Second embedded map renders independently in the same process.
    let rgba_b = map_b.render_rgba8_vec().unwrap();
    let center_b = (8 * 16 + 8) * 4;
    assert!(rgba_b[center_b + 1] >= 240 && rgba_b[center_b + 2] >= 240 && rgba_b[center_b] <= 15);

    // Camera and paint mutations on `borrowed_a`.
    borrowed_a
        .set_camera(CameraState {
            latitude: 10.0,
            longitude: 20.0,
            zoom: 4.0,
            bearing_deg: 5.0,
            pitch_deg: 10.0,
        })
        .unwrap();
    assert!((borrowed_a.camera().unwrap().zoom - 4.0).abs() < 1e-6);
    borrowed_a
        .set_paint_property_json("bg", "background-color", r##""#ffffff""##)
        .unwrap();
    borrowed_a.resize(24, 24, 1.5).unwrap();
    assert_eq!(borrowed_a.dimensions(), (24, 24));

    unsafe { NativeMap::destroy_raw_handle(raw_a) }.unwrap();
    unsafe { NativeMap::destroy_raw_handle(0) }.unwrap();
    assert!(unsafe { NativeMap::borrow_raw(0) }.is_err());
}
