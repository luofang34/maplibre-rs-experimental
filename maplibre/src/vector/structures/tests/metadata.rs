#![allow(clippy::expect_used)]

use super::super::{kind, number, StructureKind};
use crate::style::layer::StyleLayer;

fn layer(metadata: serde_json::Value) -> StyleLayer {
    serde_json::from_value(serde_json::json!({
        "id": "road", "type": "line", "metadata": metadata,
    }))
    .expect("layer metadata")
}

#[test]
fn string_structure_settings_retain_their_meaning() {
    let bridge = layer(serde_json::json!({
        "maplibre-rs:terrain-structure": "bridge",
        "maplibre-rs:structure-clearance-meters": "8.5",
    }));
    assert!(matches!(kind(&bridge), Some(StructureKind::Bridge)));
    assert_eq!(
        number(&bridge, "maplibre-rs:structure-clearance-meters"),
        Some(8.5)
    );
    let tunnel = layer(serde_json::json!({"maplibre-rs:terrain-structure": "tunnel"}));
    assert!(matches!(kind(&tunnel), Some(StructureKind::Tunnel)));
}

#[test]
fn structure_elevations_accept_json_numbers() {
    let bridge = layer(serde_json::json!({
        "maplibre-rs:terrain-structure": "bridge",
        "maplibre-rs:structure-clearance-meters": 8.5,
        "maplibre-rs:structure-elevation-meters": 250,
        "app:details": {"surveyed": true},
    }));
    assert!(matches!(kind(&bridge), Some(StructureKind::Bridge)));
    assert_eq!(
        number(&bridge, "maplibre-rs:structure-clearance-meters"),
        Some(8.5)
    );
    assert_eq!(
        number(&bridge, "maplibre-rs:structure-elevation-meters"),
        Some(250.0)
    );
}

#[test]
fn invalid_structure_settings_do_not_enable_profiles_or_elevations() {
    for value in [
        serde_json::json!(null),
        serde_json::json!(true),
        serde_json::json!([]),
        serde_json::json!({"height": 6}),
        serde_json::json!("NaN"),
        serde_json::json!("inf"),
        serde_json::json!("unknown"),
    ] {
        let invalid = layer(serde_json::json!({
            "maplibre-rs:terrain-structure": value,
            "maplibre-rs:structure-clearance-meters": value,
        }));
        assert!(kind(&invalid).is_none());
        assert!(number(&invalid, "maplibre-rs:structure-clearance-meters").is_none());
    }
}
