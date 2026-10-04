use maplibre::style::{
    expression::{FeatureProperties, Value},
    property::{StyleProperty, TextField},
};

use super::*;

/// The image `icon-image` names for a feature with `properties`.
fn image(icon: &serde_json::Value, properties: &[(&str, &str)]) -> String {
    let properties: FeatureProperties = properties
        .iter()
        .map(|(key, value)| ((*key).to_owned(), Value::String((*value).to_owned())))
        .collect();
    StyleProperty::<TextField>::parse(icon)
        .evaluate_for(&properties, 14.0)
        .map(|text| text.0)
        .unwrap_or_default()
}

#[test]
fn a_feature_names_its_routes_shield_by_whatever_schema_carries_it() {
    let first = openmaptiles_shield_image();
    for (properties, expected) in [
        (
            vec![
                ("route_1", "US:NJ:CR=609"),
                ("network", "us-state"),
                ("ref", "609"),
            ],
            "roadshield:US:NJ:CR=609",
        ),
        (
            vec![("route_1_network", "US:NJ:CR"), ("route_1_ref", "609")],
            "roadshield:US:NJ:CR=609",
        ),
        (
            vec![("network", "us-interstate"), ("ref", "287")],
            "roadshield:US:I=287",
        ),
        (
            vec![("network", "us-highway"), ("ref", "1")],
            "roadshield:US:US=1",
        ),
        (
            vec![("network", "us-state"), ("ref", "609")],
            "roadshield:=609",
        ),
        (
            vec![("network", "gb-motorway"), ("ref", "M25")],
            "roadshield:=M25",
        ),
        (vec![("network", "us-interstate")], ""),
    ] {
        assert_eq!(image(&first, &properties), expected, "{properties:?}");
    }
}

#[test]
fn later_routes_of_a_road_have_layers_of_their_own() {
    let second = openmaptiles_route_shield_image(2);
    let road = [
        ("route_1", "US:US=1"),
        ("route_2", "US:US=9"),
        ("network", "us-highway"),
        ("ref", "1"),
    ];
    assert_eq!(image(&second, &road), "roadshield:US:US=9");
    assert_eq!(
        image(
            &second,
            &[("route_2_network", "US:NJ"), ("route_2_ref", "27")]
        ),
        "roadshield:US:NJ=27"
    );
    // The class and ref stand only for the first route.
    assert_eq!(
        image(&second, &[("network", "us-highway"), ("ref", "1")]),
        ""
    );
}
