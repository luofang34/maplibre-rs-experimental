//! A label's image names are found however its expressions compute them: names read from the
//! feature, built by `concat`, and every fallback of a `coalesce`, in icons and in text.

use super::*;

/// Roads carrying a network and a ref.
pub(super) fn roads() -> Vec<u8> {
    let string = |value: &str| geozero::mvt::tile::Value {
        string_value: Some(value.to_owned()),
        ..Default::default()
    };
    geozero::mvt::Tile {
        layers: vec![geozero::mvt::tile::Layer {
            version: 2,
            name: "roads".into(),
            keys: vec!["network".into(), "ref".into()],
            values: vec![
                string("US:I"),
                string("287"),
                string("US:NJ:CR"),
                string("609"),
            ],
            features: vec![
                geozero::mvt::tile::Feature {
                    r#type: Some(1),
                    tags: vec![0, 0, 1, 1],
                    geometry: vec![9, 4096, 4096],
                    ..Default::default()
                },
                geozero::mvt::tile::Feature {
                    r#type: Some(1),
                    tags: vec![0, 2, 1, 3],
                    geometry: vec![9, 2048, 2048],
                    ..Default::default()
                },
            ],
            ..Default::default()
        }],
    }
    .encode_to_vec()
}

pub(super) fn style(icon: serde_json::Value) -> Style {
    serde_json::from_value(serde_json::json!({"version":8,"sources":{},
        "layers":[{"id":"shield","type":"symbol","source":"map","source-layer":"roads",
            "layout":{"icon-image":icon,
                "text-field":["format",["image",["concat","badge:",["get","ref"]]],{}]}}]}))
    .expect("style")
}

fn names(style: &Style) -> Vec<String> {
    let (_, icons) = requests(&style.layers, &roads(), 14.0);
    let mut icons: Vec<String> = icons.into_iter().collect();
    icons.sort();
    icons
}

#[test]
fn computed_names_and_every_fallback_of_a_coalesce_are_asked_for() {
    let style = style(serde_json::json!([
        "coalesce",
        [
            "image",
            ["concat", "shield:", ["get", "network"], "=", ["get", "ref"]]
        ],
        ["image", "generic-shield"]
    ]));
    assert_eq!(
        names(&style),
        [
            "badge:287",
            "badge:609",
            "generic-shield",
            "shield:US:I=287",
            "shield:US:NJ:CR=609"
        ]
    );
}

#[test]
fn a_name_read_from_the_feature_is_asked_for() {
    let style = style(serde_json::json!(["concat", "shield:", ["get", "ref"]]));
    assert!(names(&style).contains(&"shield:287".to_owned()));
}
