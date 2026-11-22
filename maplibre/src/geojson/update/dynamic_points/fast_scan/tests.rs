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
