#![allow(clippy::expect_used)]

use super::{super::wire::RawFeatureId, try_fast_scan_stationary_bytes, FastColumnStore};

#[test]
fn fast_scan_skips_unknown_metadata_keys_and_clears_omitted_columns() {
    let mut store = FastColumnStore {
        keys: vec!["temp".to_owned(), "radius".to_owned()],
        ..Default::default()
    };
    let expected = vec![(
        RawFeatureId::UInt(1),
        [12.5_f64.to_bits(), 55.25_f64.to_bits()],
    )];

    let payload_one = br#"{
        "type": "FeatureCollection",
        "features": [{
            "type": "Feature",
            "id": 1,
            "geometry": { "type": "Point", "coordinates": [12.5, 55.25] },
            "properties": {
                "id": "sensor-1",
                "name": "North Station",
                "meta": { "tags": ["a", "b"], "nested": { "ok": true } },
                "temp": 21.5,
                "radius": 6.0
            }
        }]
    }"#;
    assert!(try_fast_scan_stationary_bytes(
        payload_one,
        &expected,
        &mut store,
    ));
    assert_eq!(store.values[0][0], Some(21.5));
    assert_eq!(store.values[0][1], Some(6.0));

    let payload_two = br#"{
        "type": "FeatureCollection",
        "features": [{
            "type": "Feature",
            "id": 1,
            "geometry": { "type": "Point", "coordinates": [12.5, 55.25] },
            "properties": {
                "id": "sensor-1",
                "temp": 18.0
            }
        }]
    }"#;
    assert!(try_fast_scan_stationary_bytes(
        payload_two,
        &expected,
        &mut store,
    ));
    assert_eq!(store.values[0][0], Some(18.0));
    assert_eq!(
        store.values[0][1], None,
        "omitted property must reset to None rather than keeping stale value"
    );
}

#[test]
fn fast_scan_rejects_excessive_nesting_depth_cleanly() {
    let mut store = FastColumnStore {
        keys: vec!["temp".to_owned()],
        ..Default::default()
    };
    let expected = vec![(
        RawFeatureId::UInt(1),
        [1.0_f64.to_bits(), 2.0_f64.to_bits()],
    )];
    let mut payload =
        br#"{"type":"FeatureCollection","features":[{"type":"Feature","id":1,"geometry":{"type":"Point","coordinates":[1,2]},"properties":{"deep":"#.to_vec();
    payload.extend(std::iter::repeat_n(b'[', 70));
    payload.push(b'0');
    payload.extend(std::iter::repeat_n(b']', 70));
    payload.extend_from_slice(br#","temp":3}}]}"#);

    assert!(!try_fast_scan_stationary_bytes(
        &payload, &expected, &mut store,
    ));
}

#[test]
fn collect_fast_ramp_keys_accepts_s6_coalesce_circle_ramps() {
    let style: crate::style::Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "sources": {
            "sensors": {
                "type": "geojson",
                "data": {"type": "FeatureCollection", "features": []}
            }
        },
        "layers": [{
            "id": "pulses",
            "type": "circle",
            "source": "sensors",
            "paint": {
                "circle-radius": ["interpolate", ["linear"], ["coalesce", ["get", "load"], 0.5], 0, 4, 1, 13],
                "circle-color": ["interpolate", ["linear"], ["coalesce", ["get", "load"], 0.5], 0, "#22d3ee", 1, "#f43f5e"],
                "circle-opacity": 0.88
            }
        }]
    }))
    .expect("style");

    let mut keys = Vec::new();
    assert!(super::collect_fast_ramp_keys(&style, "sensors", &mut keys));
    assert_eq!(keys, vec!["load".to_owned()]);
}
