#![allow(clippy::expect_used, clippy::panic)]
use crate::style::Style;
#[test]
fn serialization_preserves_document_order_without_private_index_fields() {
    let style: Style =
        serde_json::from_value(serde_json::json!({"version":8,"sources":{},"layers":[
            {"id":"land","type":"background"},{"id":"water","type":"fill"},
            {"id":"tunnel","type":"line"},{"id":"road","type":"line"},{"id":"bridge","type":"line"}
        ]}))
        .expect("style");
    let document = serde_json::to_value(&style).expect("document");
    assert!(document["layers"][2].get("index").is_none());
    let roundtrip: Style = serde_json::from_value(document).expect("roundtrip");
    assert_eq!(
        roundtrip
            .layers
            .iter()
            .map(|layer| (layer.id.as_str(), layer.index))
            .collect::<Vec<_>>(),
        [
            ("land", 0),
            ("water", 1),
            ("tunnel", 2),
            ("road", 3),
            ("bridge", 4)
        ]
    );
}
