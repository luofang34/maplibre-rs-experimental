#![allow(clippy::expect_used, clippy::panic)]

use serde_json::json;

use super::{
    Color, EvaluationContext, EvaluationError, Expression, FeatureProperties, LegacyPropertySpec,
    PropertyKind, Type, Value,
};

fn properties() -> FeatureProperties {
    FeatureProperties::from([
        ("width".to_string(), Value::Number(7.0)),
        ("kind".to_string(), Value::from("airport")),
        ("mag".to_string(), Value::Number(2.5)),
        ("name".to_string(), Value::from("Bern")),
    ])
}

fn eval(expression: serde_json::Value, zoom: f64) -> Result<Value, EvaluationError> {
    let properties = properties();
    Expression::parse(&expression)
        .expect("expression parses")
        .evaluate(&EvaluationContext::for_feature(zoom, &properties))
}

fn number(expression: serde_json::Value, zoom: f64) -> f64 {
    eval(expression, zoom)
        .expect("evaluates")
        .as_number()
        .expect("a number")
}

#[test]
fn property_reads_keep_their_types() {
    assert_eq!(eval(json!(["get", "width"]), 0.0), Ok(Value::Number(7.0)));
    assert_eq!(
        eval(json!(["get", "kind"]), 0.0),
        Ok(Value::from("airport"))
    );
    assert_eq!(eval(json!(["get", "missing"]), 0.0), Ok(Value::Null));
    assert_eq!(eval(json!(["has", "kind"]), 0.0), Ok(Value::Bool(true)));
    assert_eq!(
        eval(json!(["typeof", ["get", "width"]]), 0.0),
        Ok(Value::from("number"))
    );
}

#[test]
fn equality_is_strict_about_types() {
    assert_eq!(
        eval(json!(["==", ["get", "width"], 7]), 0.0),
        Ok(Value::Bool(true))
    );
    assert_eq!(
        eval(json!(["==", ["get", "width"], "7"]), 0.0),
        Ok(Value::Bool(false))
    );
    assert_eq!(
        eval(json!(["!=", ["get", "kind"], "vor"]), 0.0),
        Ok(Value::Bool(true))
    );
    assert!(matches!(
        eval(json!(["<", ["get", "kind"], 3]), 0.0),
        Err(EvaluationError::Expected { .. })
    ));
    assert!(matches!(
        eval(json!(["<", ["get", "kind"], ["get", "width"]]), 0.0),
        Err(EvaluationError::Incomparable { .. })
    ));
    assert!(
        Expression::parse(&json!(["==", 1, "a"])).is_err(),
        "known mismatched types"
    );
}

#[test]
fn ramps_follow_gl_js() {
    assert_eq!(
        number(
            json!(["interpolate", ["linear"], ["zoom"], 0, 2, 10, 12]),
            5.0
        ),
        7.0
    );
    let exponential = number(
        json!([
            "interpolate",
            ["exponential", 2],
            ["get", "mag"],
            0,
            0,
            5,
            31
        ]),
        0.0,
    );
    let expected = 31.0 * (2.0_f64.powf(2.5) - 1.0) / (2.0_f64.powf(5.0) - 1.0);
    assert!((exponential - expected).abs() < 1e-9);
    assert_eq!(
        number(json!(["step", ["get", "mag"], 1, 2, 5, 3, 9]), 0.0),
        5.0
    );
    assert_eq!(number(json!(["step", ["zoom"], 1, 2, 5]), 1.0), 1.0);
    let eased = number(
        json!([
            "interpolate",
            ["cubic-bezier", 0.42, 0.0, 0.58, 1.0],
            ["zoom"],
            0,
            0,
            10,
            100
        ]),
        5.0,
    );
    assert!(
        (eased - 50.0).abs() < 1e-6,
        "a symmetric ease passes through the middle: {eased}"
    );
}

#[test]
fn a_ramp_over_a_non_number_is_an_error_not_a_panic() {
    let zero = json!(["-", ["get", "width"], 7]);
    assert_eq!(
        eval(
            json!(["interpolate", ["linear"], ["/", zero, zero], 1, 1, 2, 2]),
            0.0
        ),
        Err(EvaluationError::InputNotANumber)
    );
    assert!(
        Expression::parse(&json!(["step", ["/", 0, 0], 1, 2, 2])).is_err(),
        "a constant ramp over a non-number fails while parsing"
    );
    assert_eq!(
        eval(
            json!(["step", ["/", 0, ["get", "missing_number"]], 1, 2, 2]),
            0.0
        ),
        Err(EvaluationError::Expected {
            expected: Type::Number,
            found: Type::Null
        })
    );
}

#[test]
fn branches_and_lookups() {
    assert_eq!(
        number(
            json!(["match", ["get", "kind"], ["vor", "airport"], 8, 2]),
            0.0
        ),
        8.0
    );
    assert_eq!(
        number(json!(["match", ["get", "width"], "7", 8, 2]), 0.0),
        2.0
    );
    assert_eq!(
        number(json!(["case", [">", ["get", "mag"], 2], 1, 0]), 0.0),
        1.0
    );
    assert_eq!(number(json!(["coalesce", ["get", "missing"], 4]), 0.0), 4.0);
    assert_eq!(
        eval(
            json!(["let", "m", ["get", "mag"], ["*", ["var", "m"], 2]]),
            0.0
        ),
        Ok(Value::Number(5.0))
    );
    assert_eq!(
        eval(json!(["in", "er", ["get", "name"]]), 0.0),
        Ok(Value::Bool(true))
    );
    assert_eq!(number(json!(["index-of", "n", ["get", "name"]]), 0.0), 3.0);
    assert_eq!(
        eval(json!(["slice", ["get", "name"], 1, 3]), 0.0),
        Ok(Value::from("er"))
    );
    assert_eq!(number(json!(["length", ["get", "name"]]), 0.0), 4.0);
}

#[test]
fn arithmetic_strings_and_colors() {
    assert_eq!(number(json!(["+", 1, 2, 3]), 0.0), 6.0);
    assert_eq!(number(json!(["-", 10, ["get", "width"]]), 0.0), 3.0);
    assert_eq!(number(json!(["-", 4]), 0.0), -4.0);
    assert_eq!(number(json!(["^", 2, 10]), 0.0), 1024.0);
    assert_eq!(number(json!(["round", -2.5]), 0.0), -3.0);
    assert_eq!(
        eval(
            json!(["concat", ["get", "name"], " ", ["get", "width"]]),
            0.0
        ),
        Ok(Value::from("Bern 7"))
    );
    assert_eq!(eval(json!(["upcase", "abc"]), 0.0), Ok(Value::from("ABC")));
    assert_eq!(
        eval(json!(["to-color", "#ff0000"]), 0.0),
        Ok(Value::Color(Color::new(1.0, 0.0, 0.0, 1.0)))
    );
    assert_eq!(
        eval(json!(["to-rgba", ["rgb", 0, 255, 0]]), 0.0),
        Ok(Value::Array(vec![
            Value::Number(0.0),
            Value::Number(255.0),
            Value::Number(0.0),
            Value::Number(1.0)
        ]))
    );
    assert!(matches!(
        eval(json!(["to-color", ["get", "name"]]), 0.0),
        Err(EvaluationError::InvalidColor { .. })
    ));
    assert_eq!(
        eval(json!(["to-number", "  12 "]), 0.0),
        Ok(Value::Number(12.0))
    );
    assert_eq!(eval(json!(["to-string", 1.5]), 0.0), Ok(Value::from("1.5")));
}

#[test]
fn classification_sees_through_variables() {
    let zoom_only = Expression::parse(&json!(["interpolate", ["linear"], ["zoom"], 0, 1, 10, 2]))
        .expect("parses");
    assert!(zoom_only.is_feature_constant() && !zoom_only.is_zoom_constant());
    let feature =
        Expression::parse(&json!(["let", "w", ["get", "width"], ["var", "w"]])).expect("parses");
    assert!(!feature.is_feature_constant() && feature.is_zoom_constant());
    let constant = Expression::parse(&json!(["+", 1, 2])).expect("parses");
    assert!(constant.is_constant());
    assert!(matches!(constant, Expression::Folded { .. }));
    assert_eq!(constant.output_type(), Type::Number);
}

#[test]
fn unknown_operators_and_bad_shapes_are_parse_errors() {
    let error = Expression::parse(&json!(["frobnicate", 1])).expect_err("unknown operator");
    assert!(error.is_unknown_operator(), "{error}");
    assert!(Expression::parse(&json!(["step", ["zoom"], 1, 5, 2, 3, 3])).is_err());
    assert!(Expression::parse(&json!(["match", ["get", "kind"], 1.5, "a", "b"])).is_err());
    assert!(Expression::parse(&json!(["var", "nope"])).is_err());
    assert!(
        Expression::parse(&json!({"stops": []})).is_err(),
        "bare objects are not expressions"
    );
}

#[test]
fn legacy_functions_lower_into_expressions() {
    let spec = LegacyPropertySpec::interpolated(PropertyKind::Number);
    let parse = |json: serde_json::Value| Expression::parse_property(&json, &spec).expect("parses");
    let evaluate = |expression: &Expression, zoom: f64| {
        let properties = properties();
        expression
            .evaluate(&EvaluationContext::for_feature(zoom, &properties))
            .expect("evaluates")
            .as_number()
            .expect("a number")
    };
    assert_eq!(
        evaluate(&parse(json!({"stops": [[2, 1], [4, 3]]})), 3.0),
        2.0
    );
    assert_eq!(
        evaluate(
            &parse(json!({"type": "interval", "stops": [[2, 1], [4, 3]]})),
            3.0
        ),
        1.0
    );
    assert_eq!(
        evaluate(
            &parse(json!({"property": "mag", "stops": [[0, 0], [5, 10]]})),
            0.0
        ),
        5.0
    );
    assert_eq!(
        evaluate(
            &parse(
                json!({"property": "kind", "type": "categorical", "stops": [["airport", 9], ["vor", 3]]})
            ),
            0.0
        ),
        9.0
    );
    assert_eq!(
        evaluate(
            &parse(json!({"type": "identity", "property": "missing", "default": 2})),
            0.0
        ),
        2.0
    );
    let categorical =
        parse(json!({"property": "kind", "type": "categorical", "stops": [["vor", 3]]}));
    let properties = properties();
    assert_eq!(
        categorical.evaluate(&EvaluationContext::for_feature(0.0, &properties)),
        Ok(Value::Null),
        "no default means no value, so the caller's default applies"
    );
    let composite = parse(json!({"property": "mag", "stops": [
        [{"zoom": 0, "value": 0}, 0.0], [{"zoom": 0, "value": 5}, 10.0],
        [{"zoom": 2, "value": 0}, 20.0], [{"zoom": 2, "value": 5}, 30.0]
    ]}));
    assert_eq!(evaluate(&composite, 1.0), 15.0);
    let color_spec = LegacyPropertySpec::interpolated(PropertyKind::Color);
    let color = Expression::parse_property(&json!("red"), &color_spec).expect("a colour literal");
    assert!(matches!(
        color,
        Expression::Folded {
            value: Value::Color(_),
            ..
        }
    ));
}

#[test]
fn legacy_filters_lower_into_expressions() {
    let properties = properties();
    let passes = |json: serde_json::Value| {
        Expression::parse_filter(&json)
            .expect("filter parses")
            .evaluate(&EvaluationContext::for_feature(8.0, &properties))
            .expect("evaluates")
            .as_bool()
            .expect("a boolean")
    };
    assert!(passes(json!(["==", "kind", "airport"])));
    assert!(!passes(json!(["==", "width", "7"])));
    assert!(passes(json!(["in", "kind", "vor", "airport"])));
    assert!(passes(json!(["!in", "kind", "vor"])));
    assert!(passes(json!(["all", ["has", "kind"], ["!has", "missing"]])));
    assert!(passes(json!(["none", ["==", "kind", "vor"]])));
    assert!(passes(json!(["any", [">", "mag", 5], ["<", "mag", 3]])));
    assert!(!passes(json!(["==", "missing", null])));
}
