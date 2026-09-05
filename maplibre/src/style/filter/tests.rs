#![allow(clippy::expect_used, clippy::panic)]

use std::collections::HashMap;

use serde_json::{json, Value};

use super::{Comparison, FeatureContext, Filter, FilterError, GeometryType, Operand};

fn low_airway() -> HashMap<String, Value> {
    HashMap::from([
        ("level".to_string(), json!("low")),
        ("altitude".to_string(), json!(18000)),
        ("name".to_string(), json!("V-12")),
    ])
}

fn passes(filter: Value, properties: &HashMap<String, Value>) -> bool {
    Filter::parse(&filter)
        .expect("filter parses")
        .evaluate(&FeatureContext {
            properties,
            geometry_type: GeometryType::LineString,
            id: Some(json!(7)),
            zoom: 8.0,
        })
}

#[test]
fn legacy_and_expression_forms_parse_to_the_same_comparison() {
    let expected = Filter::Compare {
        operator: Comparison::Equal,
        left: Operand::Get("level".to_string()),
        right: Operand::Literal(json!("low")),
    };
    assert_eq!(
        Filter::parse(&json!(["==", "level", "low"])),
        Ok(expected.clone())
    );
    assert_eq!(
        Filter::parse(&json!(["==", ["get", "level"], "low"])),
        Ok(expected)
    );
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
    let properties = HashMap::new();
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
    assert!(passes(Value::Null, &properties));
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
    assert_eq!(
        Filter::parse(&json!(["within", {"type": "Polygon", "coordinates": []}])),
        Err(FilterError::UnsupportedOperator {
            operator: "within".to_string()
        })
    );
    assert_eq!(
        Filter::parse(&json!(["==", ["global-state", "level"], "low"])),
        Err(FilterError::UnsupportedOperator {
            operator: "global-state".to_string()
        })
    );
    assert_eq!(
        Filter::parse(&json!(["==", ["get", "a"], ["get", "b"], ["collator", {}]])),
        Err(FilterError::Malformed {
            operator: "==".to_string(),
            expected: "two operands"
        })
    );
    assert_eq!(
        Filter::parse(&json!({"type": "identity"})),
        Err(FilterError::NotAFilter {
            found: "object".to_string()
        })
    );
    assert!(matches!(
        Filter::parse(&json!(["match", ["get", "level"], "low"])),
        Err(FilterError::Malformed { .. })
    ));
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
