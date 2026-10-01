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

#[test]
fn a_layer_with_ref_takes_the_definition_of_the_layer_it_names() {
    let style: Style =
        serde_json::from_value(serde_json::json!({"version":8,"sources":{},"layers":[
            {"id":"a","type":"symbol","source":"s","source-layer":"poi","minzoom":3,
             "filter":["==","k","v"],"layout":{"icon-image":"x"}},
            {"id":"b","ref":"a","paint":{"icon-opacity":0.5}}
        ]}))
        .expect("style");
    let (a, b) = (&style.layers[0], &style.layers[1]);
    assert_eq!(b.type_, a.type_);
    assert_eq!(b.source, a.source);
    assert_eq!(b.source_layer, a.source_layer);
    assert_eq!(b.minzoom, Some(3.0));
    assert_eq!(b.filter, a.filter);
    assert_eq!(b.index, 1);
}
