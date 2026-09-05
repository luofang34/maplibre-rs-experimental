#![allow(clippy::expect_used, clippy::panic)]

use serde_json::{json, Value as Json};

use super::{properties_from_json, FeatureContext, Filter, FilterError, GeometryType};
use crate::style::expression::{FeatureProperties, Value};

fn low_airway() -> FeatureProperties {
    properties_from_json(Some(
        &json!({"level": "low", "altitude": 18000, "name": "V-12"}),
    ))
}

fn passes(filter: Json, properties: &FeatureProperties) -> bool {
    Filter::parse(&filter)
        .expect("filter parses")
        .evaluate(&FeatureContext {
            properties,
            geometry_type: GeometryType::LineString,
            id: Some(Value::Number(7.0)),
            zoom: 8.0,
        })
}

#[test]
fn legacy_and_expression_forms_parse_to_the_same_comparison() {
    let legacy = Filter::parse(&json!(["==", "level", "low"])).expect("legacy filter parses");
    let expression =
        Filter::parse(&json!(["==", ["get", "level"], "low"])).expect("expression filter parses");
    assert_eq!(legacy, expression);
}

#[test]
fn equality_selects_the_same_features_in_both_forms() {
    let properties = low_airway();
    assert!(passes(json!(["==", "level", "low"]), &properties));
    assert!(passes(json!(["==", ["get", "level"], "low"]), &properties));
    assert!(!passes(json!(["==", "level", "high"]), &properties));
    assert!(!passes(
        json!(["==", ["get", "level"], "high"]),
        &properties
    ));
    assert!(!passes(json!(["!=", "level", "low"]), &properties));
    assert!(!passes(json!(["!=", ["get", "level"], "low"]), &properties));
}

#[test]
fn a_missing_property_is_unequal_to_everything() {
    let properties = FeatureProperties::new();
    assert!(!passes(json!(["==", "level", "low"]), &properties));
    assert!(passes(json!(["!=", "level", "low"]), &properties));
    assert!(passes(json!(["!=", ["get", "level"], "low"]), &properties));
    assert!(!passes(json!(["<", "altitude", 1]), &properties));
    assert!(!passes(json!(["in", "level", "low", "high"]), &properties));
    assert!(passes(json!(["!in", "level", "low", "high"]), &properties));
    assert!(!passes(json!(["has", "level"]), &properties));
    assert!(passes(json!(["!has", "level"]), &properties));
}

#[test]
fn values_compare_by_type() {
    let properties = low_airway();
    assert!(passes(json!(["==", "altitude", 18000]), &properties));
    assert!(passes(
        json!(["==", ["get", "altitude"], 18000.0]),
        &properties
    ));
    assert!(!passes(json!(["==", "altitude", "18000"]), &properties));
    assert!(passes(
        json!([">=", ["get", "altitude"], 18000]),
        &properties
    ));
    assert!(!passes(
        json!([">", ["get", "altitude"], 18000]),
        &properties
    ));
    assert!(!passes(json!([">", ["get", "level"], 1]), &properties));
    assert!(passes(json!(["<", ["get", "level"], "z"]), &properties));
    assert!(passes(
        json!([
            "==",
            ["to-number", ["get", "altitude"]],
            ["to-number", "18000"]
        ]),
        &properties
    ));
    assert!(passes(
        json!(["==", ["to-string", ["get", "altitude"]], "18000"]),
        &properties
    ));
}

#[test]
fn legacy_membership_and_existence_operators() {
    let properties = low_airway();
    assert!(passes(json!(["in", "level", "high", "low"]), &properties));
    assert!(!passes(json!(["!in", "level", "high", "low"]), &properties));
    assert!(passes(json!(["has", "level"]), &properties));
    assert!(!passes(json!(["!has", "level"]), &properties));
    assert!(passes(json!(["has", "$type"]), &properties));
    assert!(passes(json!(["has", "$id"]), &properties));
    assert!(passes(json!(["in", "$type", "LineString"]), &properties));
    assert!(passes(json!(["==", "$type", "LineString"]), &properties));
    assert!(!passes(json!(["==", "$type", "Point"]), &properties));
    assert!(passes(json!(["==", "$id", 7]), &properties));
}

#[test]
fn legacy_combinators() {
    let properties = low_airway();
    assert!(passes(
        json!(["all", ["==", "level", "low"], [">", "altitude", 1000]]),
        &properties
    ));
    assert!(!passes(
        json!(["all", ["==", "level", "low"], [">", "altitude", 100000]]),
        &properties
    ));
    assert!(passes(
        json!(["any", ["==", "level", "high"], ["==", "name", "V-12"]]),
        &properties
    ));
    assert!(!passes(
        json!(["none", ["==", "level", "low"]]),
        &properties
    ));
    assert!(passes(
        json!(["none", ["==", "level", "high"]]),
        &properties
    ));
    assert!(passes(json!(["all"]), &properties));
    assert!(!passes(json!(["any"]), &properties));
    assert!(passes(json!([]), &properties));
    assert!(passes(json!(null), &properties));
}

#[test]
fn expression_operators() {
    let properties = low_airway();
    assert!(passes(
        json!(["!", ["==", ["get", "level"], "high"]]),
        &properties
    ));
    assert!(passes(
        json!(["all", ["has", "level"], ["!", ["has", "missing"]]]),
        &properties
    ));
    assert!(passes(
        json!(["in", ["get", "level"], ["literal", ["low", "high"]]]),
        &properties
    ));
    assert!(passes(json!(["in", "V", ["get", "name"]]), &properties));
    assert!(!passes(json!(["in", "X", ["get", "name"]]), &properties));
    assert!(passes(
        json!(["match", ["get", "level"], ["low", "medium"], true, false]),
        &properties
    ));
    assert!(!passes(
        json!(["match", ["get", "level"], "high", true, false]),
        &properties
    ));
    assert!(passes(
        json!(["case", ["==", ["get", "level"], "low"], true, false]),
        &properties
    ));
    assert!(passes(
        json!(["==", ["geometry-type"], "LineString"]),
        &properties
    ));
    assert!(passes(json!(["==", ["id"], 7]), &properties));
    assert!(passes(json!(["<=", ["zoom"], 8]), &properties));
    assert!(!passes(json!(["<", ["zoom"], 8]), &properties));
    assert!(passes(json!(["boolean", true]), &properties));
    assert!(!passes(json!(["literal", false]), &properties));
    assert!(!passes(json!(false), &properties));
}

#[test]
fn unsupported_operators_are_errors_not_guesses() {
    for filter in [
        json!(["within", {"type": "Polygon", "coordinates": []}]),
        json!(["==", ["feature-state", "level"], "low"]),
        json!(["==", ["get", "a"], ["get", "b"], ["collator", {}]]),
        json!(["match", ["get", "level"], "low"]),
    ] {
        assert!(
            matches!(Filter::parse(&filter), Err(FilterError::Invalid { .. })),
            "{filter} must be rejected"
        );
    }
    assert_eq!(
        Filter::parse(&json!({"type": "identity"})),
        Err(FilterError::NotAFilter {
            found: "object".to_string()
        })
    );
}

#[test]
fn geometry_types_follow_mvt_and_geojson_names() {
    assert_eq!(GeometryType::from_mvt(1), GeometryType::Point);
    assert_eq!(GeometryType::from_mvt(2), GeometryType::LineString);
    assert_eq!(GeometryType::from_mvt(3), GeometryType::Polygon);
    assert_eq!(GeometryType::from_mvt(9), GeometryType::Unknown);
    assert_eq!(
        GeometryType::from_geojson("MultiPolygon"),
        GeometryType::Polygon
    );
    assert_eq!(
        GeometryType::from_geojson("MultiPoint"),
        GeometryType::Point
    );
    assert_eq!(
        GeometryType::from_geojson("GeometryCollection"),
        GeometryType::Unknown
    );
}
