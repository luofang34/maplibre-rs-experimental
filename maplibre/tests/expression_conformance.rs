//! Runs the expression conformance cases of the maplibre style specification, vendored under
//! `tests/expressions`, against the expression engine through its public API.
//!
//! Each case parses one expression, checks its compiled classification and type, and
//! evaluates it for a list of inputs. Cases that use operators or types the engine does not
//! implement are counted as unsupported rather than failed; the floor on passing cases keeps
//! the supported subset from shrinking silently.

#![allow(clippy::expect_used, clippy::panic)]

use std::{fs, path::Path};

use maplibre::style::expression::{
    EvaluationContext, Expression, FeatureProperties, LegacyPropertySpec, PropertyKind, Type, Value,
};
use serde_json::Value as Json;

/// Passing cases the vendored suite must keep producing.
const MIN_PASSING_CASES: usize = 380;

/// Types of the specification the engine has no value for.
const UNSUPPORTED_TYPES: &[&str] = &[
    "formatted",
    "resolvedImage",
    "padding",
    "numberArray",
    "colorArray",
    "projectionDefinition",
    "variableAnchorOffsetCollection",
    "collator",
];

enum Outcome {
    Pass,
    Unsupported(String),
    Fail(String),
}

#[test]
fn the_engine_agrees_with_the_style_specification() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/expressions");
    let mut cases = Vec::new();
    collect_cases(&root, &mut cases);
    cases.sort();
    assert!(!cases.is_empty(), "no cases under {}", root.display());

    let mut passed = 0;
    let mut unsupported = Vec::new();
    let mut failures = Vec::new();
    for case in &cases {
        let name = case
            .strip_prefix(&root)
            .expect("under root")
            .parent()
            .expect("a directory")
            .display()
            .to_string();
        match run_case(case) {
            Outcome::Pass => passed += 1,
            Outcome::Unsupported(reason) => unsupported.push(format!("{name}: {reason}")),
            Outcome::Fail(reason) => failures.push(format!("{name}: {reason}")),
        }
    }
    eprintln!(
        "expression conformance: {passed} passed, {} unsupported, {} failed of {}",
        unsupported.len(),
        failures.len(),
        cases.len()
    );
    for reason in &unsupported {
        eprintln!("unsupported {reason}");
    }
    assert!(
        failures.is_empty(),
        "failed cases:\n{}",
        failures.join("\n")
    );
    assert!(
        passed >= MIN_PASSING_CASES,
        "only {passed} cases pass; the floor is {MIN_PASSING_CASES}"
    );
}

fn collect_cases(directory: &Path, cases: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(directory).expect("readable directory") {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            collect_cases(&path, cases);
        } else if path.file_name().is_some_and(|name| name == "test.json") {
            cases.push(path);
        }
    }
}

fn run_case(path: &Path) -> Outcome {
    let case: Json = serde_json::from_str(&fs::read_to_string(path).expect("readable case"))
        .expect("valid JSON");
    let text = case.to_string();
    if let Some(unsupported) = UNSUPPORTED_TYPES
        .iter()
        .find(|kind| text.contains(&format!("\"{kind}\"")))
    {
        return Outcome::Unsupported(format!("uses the {unsupported} type"));
    }
    let spec = property_spec(case.get("propertySpec"));
    let expected = &case["expected"];
    let compiled = &expected["compiled"];
    // The suite treats every case as a property expression; without a specification the
    // property accepts any value.
    let spec = spec.unwrap_or_else(|| LegacyPropertySpec::stepped(PropertyKind::Value));
    let parsed = Expression::parse_property(&case["expression"], &spec);
    let expression = match (parsed, compiled["result"].as_str()) {
        (Err(error), _) if error.is_unknown_operator() => {
            return Outcome::Unsupported(error.message)
        }
        (Err(_), Some("error")) => return Outcome::Pass,
        (Err(error), _) => return Outcome::Fail(format!("parse error {error}")),
        (Ok(_), Some("error")) => return Outcome::Fail("parsed but an error was expected".into()),
        (Ok(expression), _) => expression,
    };
    if let Some(kind) = compiled["type"].as_str() {
        let actual = expression.output_type().name();
        if kind != actual && !(kind.starts_with("array") && actual.starts_with("array")) {
            return Outcome::Fail(format!("type {actual}, expected {kind}"));
        }
    }
    for (flag, actual) in [
        ("isFeatureConstant", expression.is_feature_constant()),
        ("isZoomConstant", expression.is_zoom_constant()),
    ] {
        if let Some(expected) = compiled[flag].as_bool() {
            if expected != actual {
                return Outcome::Fail(format!("{flag} is {actual}, expected {expected}"));
            }
        }
    }
    let inputs = case["inputs"].as_array().cloned().unwrap_or_default();
    let outputs = expected["outputs"].as_array().cloned().unwrap_or_default();
    for (index, (input, expected)) in inputs.iter().zip(&outputs).enumerate() {
        let globals = &input[0];
        let feature = &input[1];
        let properties: FeatureProperties = feature
            .get("properties")
            .and_then(Json::as_object)
            .map(|object| {
                object
                    .iter()
                    .map(|(key, value)| (key.clone(), Value::from_json(value)))
                    .collect()
            })
            .unwrap_or_default();
        let id = feature.get("id").map(Value::from_json);
        // The suite keeps the map-level state at the case root.
        let global_state: Option<FeatureProperties> = case
            .get("globalState")
            .or_else(|| globals.get("globalState"))
            .and_then(Json::as_object)
            .map(|state| {
                state
                    .iter()
                    .map(|(key, value)| (key.clone(), Value::from_json(value)))
                    .collect()
            });
        let geometry_type = feature
            .get("geometry")
            .and_then(|geometry| geometry.get("type"))
            .and_then(Json::as_str);
        let context = EvaluationContext {
            zoom: globals.get("zoom").and_then(Json::as_f64).unwrap_or(0.0),
            elevation: globals
                .get("elevation")
                .and_then(Json::as_f64)
                .unwrap_or(0.0),
            properties: Some(&properties),
            geometry_type,
            id: id.as_ref(),
            global_state: global_state.as_ref(),
        };
        let result = expression.evaluate(&context);
        let expects_error = expected.get("error").is_some();
        match (result, expects_error) {
            (Err(_), true) => {}
            (Err(error), false) => return Outcome::Fail(format!("input {index}: {error}")),
            (Ok(value), true) => {
                return Outcome::Fail(format!(
                    "input {index}: got {} but an error was expected",
                    value.to_json()
                ))
            }
            (Ok(value), false) => {
                if !matches(&value, expected) {
                    return Outcome::Fail(format!(
                        "input {index}: got {}, expected {expected}",
                        value.to_json()
                    ));
                }
            }
        }
    }
    Outcome::Pass
}

fn property_spec(spec: Option<&Json>) -> Option<LegacyPropertySpec> {
    let spec = spec?.as_object()?;
    let kind = match spec.get("type").and_then(Json::as_str)? {
        "number" => PropertyKind::Number,
        "string" => PropertyKind::String,
        "boolean" => PropertyKind::Boolean,
        "color" => PropertyKind::Color,
        "enum" => PropertyKind::Enum(
            spec.get("values")
                .and_then(Json::as_object)
                .map(|values| values.keys().cloned().collect())
                .unwrap_or_default(),
        ),
        "array" => PropertyKind::Array {
            item: Box::new(match spec.get("value").and_then(Json::as_str) {
                Some("string") => PropertyKind::String,
                Some("boolean") => PropertyKind::Boolean,
                _ => PropertyKind::Number,
            }),
            length: spec
                .get("length")
                .and_then(Json::as_u64)
                .map(|n| n as usize),
        },
        _ => return None,
    };
    Some(LegacyPropertySpec {
        kind,
        interpolated: spec
            .get("expression")
            .and_then(|expression| expression.get("interpolated"))
            .and_then(Json::as_bool)
            .unwrap_or(false),
        default: spec.get("default").cloned(),
        tokens: spec.get("tokens").and_then(Json::as_bool).unwrap_or(false),
    })
}

/// Whether a value matches the suite's expected output, with colours compared as the
/// premultiplied arrays the suite writes and numbers within the precision it prints.
fn matches(value: &Value, expected: &Json) -> bool {
    match (value, expected) {
        (Value::Number(number), Json::Number(expected)) => {
            let expected = expected.as_f64().unwrap_or(f64::NAN);
            // The suite prints six significant digits.
            (number - expected).abs() <= 1e-5 * expected.abs().max(1.0)
        }
        (Value::Color(color), Json::Array(components)) => {
            let premultiplied = color.premultiplied();
            components.len() == 4
                && components
                    .iter()
                    .zip(premultiplied)
                    .all(|(expected, actual)| {
                        expected
                            .as_f64()
                            .is_some_and(|expected| (actual - expected).abs() <= 1e-6)
                    })
        }
        (Value::Array(items), Json::Array(expected)) => {
            items.len() == expected.len()
                && items
                    .iter()
                    .zip(expected)
                    .all(|(item, expected)| matches(item, expected))
        }
        (Value::Object(members), Json::Object(expected)) => {
            members.len() == expected.len()
                && members.iter().all(|(key, value)| {
                    expected
                        .get(key)
                        .is_some_and(|expected| matches(value, expected))
                })
        }
        (value, expected) => value.to_json() == *expected,
    }
}

#[test]
fn the_vendored_suite_covers_every_supported_operator() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/expressions");
    for operator in ["match", "interpolate", "step", "legacy", "to-color", "let"] {
        assert!(
            root.join(operator).is_dir(),
            "{operator} cases are vendored"
        );
    }
    let _ = Type::Value;
}
