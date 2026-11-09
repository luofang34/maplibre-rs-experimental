use serde_json::json;

use super::{reads_identity, GEOMETRY_TYPE_PROPERTY, ID_PROPERTY};
use crate::style::{
    expression::{EvaluationContext, Expression, FeatureProperties, Value},
    layer::StyleProperty,
};

#[test]
fn only_the_operators_count_not_a_property_or_a_layer_called_id() {
    assert!(reads_identity(
        &json!({"circle-radius": ["case", ["==", ["id"], 1], 4, 2]})
    ));
    assert!(reads_identity(&json!({"text-field": ["geometry-type"]})));
    assert!(!reads_identity(
        &json!({"id": "id", "text-field": ["get", "id"]})
    ));
}

#[test]
fn the_properties_a_feature_carries_answer_both_operators_and_stay_out_of_properties() {
    let mut properties = FeatureProperties::new();
    properties.insert(ID_PROPERTY.to_owned(), Value::Number(7.0));
    properties.insert(GEOMETRY_TYPE_PROPERTY.to_owned(), Value::from("Point"));
    properties.insert("name".to_owned(), Value::from("a"));

    let text = StyleProperty::<String>::parse(&json!([
        "concat",
        ["geometry-type"],
        ["to-string", ["id"]]
    ]));
    assert_eq!(
        text.evaluate_for(&properties, 0.0).as_deref(),
        Some("Point7")
    );

    let keys = Expression::parse(&json!(["properties"]))
        .expect("parses")
        .evaluate(&EvaluationContext::for_feature(0.0, &properties));
    assert_eq!(
        keys.map(|value| value.to_json()),
        Ok(json!({"name": "a"})),
        "the renderer's own properties are not the feature's"
    );
    let queried = crate::query::source_properties(&properties);
    assert_eq!(
        queried.keys().collect::<Vec<_>>(),
        ["name"],
        "nor are they what a query returns"
    );
}
