//! Runs the expression conformance cases of the MapLibre Style Specification, vendored under
//! `tests/expressions`, against the expression engine through its public API.
//!
//! Each case parses one expression, checks its compiled classification and type, and
//! evaluates it for a list of inputs. Unsupported specification types and known deviations are
//! tracked in a ratcheted `EXPECTED_FAILURES` list mapping `<group>/<case>` to an explicit
//! reason: any unlisted failure OR any listed case that starts passing fails the test.

#![allow(clippy::expect_used, clippy::panic)]

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::Path,
};

use maplibre::style::expression::{
    EvaluationContext, Expression, FeatureProperties, LegacyPropertySpec, PropertyKind, Type,
    Value, FEATURE_STATE_PREFIX, GEOMETRY_PROPERTY,
};
use serde_json::Value as Json;

/// Passing cases the vendored suite must keep producing.
const MIN_PASSING_CASES: usize = 451;

/// Ratcheted map of exact `<group>/<case>` IDs expected not to pass, paired with explicit reasons.
const EXPECTED_FAILURES: &[(&str, &str)] = &[];

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

    let mut expected_map = BTreeMap::new();
    for (id, reason) in EXPECTED_FAILURES {
        assert!(
            !reason.trim().is_empty(),
            "EXPECTED_FAILURES entry `{id}` must have a non-empty reason"
        );
        assert!(
            expected_map.insert(*id, *reason).is_none(),
            "duplicate EXPECTED_FAILURES entry `{id}`"
        );
    }

    let mut passed = 0_usize;
    let mut expected_non_passing = 0_usize;
    let mut unexpected_failures = Vec::new();
    let mut unexpected_passes = Vec::new();
    let mut seen_ids = BTreeSet::new();

    for case in &cases {
        let name = case
            .strip_prefix(&root)
            .expect("under root")
            .parent()
            .expect("a directory")
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        seen_ids.insert(name.clone());

        let expected_reason = expected_map.get(name.as_str()).copied();
        match (run_case(case), expected_reason) {
            (Outcome::Pass, None) => passed += 1,
            (Outcome::Pass, Some(reason)) => {
                unexpected_passes.push(format!("{name} (listed as: {reason})"));
            }
            (Outcome::Unsupported(_), Some(_)) | (Outcome::Fail(_), Some(_)) => {
                expected_non_passing += 1;
            }
            (Outcome::Unsupported(reason), None) => {
                unexpected_failures.push(format!("{name}: unsupported ({reason})"));
            }
            (Outcome::Fail(reason), None) => {
                unexpected_failures.push(format!("{name}: {reason}"));
            }
        }
    }

    let stale_entries: Vec<&str> = expected_map
        .keys()
        .copied()
        .filter(|id| !seen_ids.contains(*id))
        .collect();

    eprintln!(
        "expression conformance: {passed} passed, {expected_non_passing} expected-unsupported, {} unexpected failures, {} unexpected passes of {}",
        unexpected_failures.len(),
        unexpected_passes.len(),
        cases.len()
    );

    assert!(
        stale_entries.is_empty(),
        "stale EXPECTED_FAILURES entries missing from disk:\n{}",
        stale_entries.join("\n")
    );
    assert!(
        unexpected_passes.is_empty(),
        "cases in EXPECTED_FAILURES now pass — remove them from EXPECTED_FAILURES:\n{}",
        unexpected_passes.join("\n")
    );
    assert!(
        unexpected_failures.is_empty(),
        "unexpected expression conformance failures:\n{}",
        unexpected_failures.join("\n")
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
    let spec = property_spec(case.get("propertySpec"));
    let expected = &case["expected"];
    let compiled = &expected["compiled"];
    // The suite treats every case as a property expression; without a specification the
    // property accepts any value.
    let spec = spec.unwrap_or_else(|| LegacyPropertySpec::stepped(PropertyKind::Value));
    let parsed = Expression::parse_property(&case["expression"], &spec);
    // An unknown operator passes a case expecting an error only when that error is the unknown
    // operator itself; otherwise an operator the engine lacks would pass for a type error.
    let expects_unknown = compiled["errors"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|error| error["error"].as_str())
        .any(|message| message.starts_with("Unknown expression"));
    let expression = match (parsed, compiled["result"].as_str()) {
        (Err(error), Some("error")) if !error.is_unknown_operator() || expects_unknown => {
            return Outcome::Pass;
        }
        (Err(error), _) if error.is_unknown_operator() => {
            return Outcome::Unsupported(error.message);
        }
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
        if let Some(expected) = compiled[flag].as_bool()
            && expected != actual
        {
            return Outcome::Fail(format!("{flag} is {actual}, expected {expected}"));
        }
    }
    let inputs = case["inputs"].as_array().cloned().unwrap_or_default();
    let outputs = expected["outputs"].as_array().cloned().unwrap_or_default();
    for (index, (input, expected)) in inputs.iter().zip(&outputs).enumerate() {
        let globals = &input[0];
        let feature = &input[1];
        let mut properties: FeatureProperties = feature
            .get("properties")
            .and_then(Json::as_object)
            .map(|object| {
                object
                    .iter()
                    .map(|(key, value)| (key.clone(), Value::from_json(value)))
                    .collect()
            })
            .unwrap_or_default();
        if let Some(geometry) = feature.get("geometry") {
            properties.insert(
                GEOMETRY_PROPERTY.to_owned(),
                Value::String(geometry.to_string()),
            );
        }
        if let Some(feature_state) = globals
            .get("featureState")
            .or_else(|| feature.get("state"))
            .and_then(Json::as_object)
        {
            for (key, value) in feature_state {
                properties.insert(
                    format!("{FEATURE_STATE_PREFIX}{key}"),
                    Value::from_json(value),
                );
            }
        }
        let id = feature.get("id").map(Value::from_json);
        // Per-input `globals.globalState` overrides a root `case.globalState` default.
        let global_state: Option<FeatureProperties> = globals
            .get("globalState")
            .or_else(|| case.get("globalState"))
            .and_then(Json::as_object)
            .map(|state| {
                state
                    .iter()
                    .map(|(key, value)| (key.clone(), Value::from_json(value)))
                    .collect()
            });
        let available_images: HashMap<String, ()> = globals
            .get("availableImages")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .filter_map(Json::as_str)
            .map(|name| (name.to_owned(), ()))
            .collect();
        let geometry_type = feature
            .get("geometry")
            .and_then(|geometry| geometry.get("type"))
            .and_then(Json::as_str);
        let accumulated = globals.get("accumulated").map(Value::from_json);
        let context = EvaluationContext {
            zoom: globals.get("zoom").and_then(Json::as_f64).unwrap_or(0.0),
            elevation: globals
                .get("elevation")
                .and_then(Json::as_f64)
                .unwrap_or(0.0),
            heatmap_density: globals
                .get("heatmapDensity")
                .and_then(Json::as_f64)
                .unwrap_or(0.0),
            line_progress: globals
                .get("lineProgress")
                .and_then(Json::as_f64)
                .unwrap_or(0.0),
            properties: Some(&properties),
            geometry_type,
            id: id.as_ref(),
            accumulated: accumulated.as_ref(),
            global_state: global_state.as_ref(),
            available_images: Some(&available_images),
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
                ));
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
        "formatted" => PropertyKind::Formatted,
        "resolvedImage" => PropertyKind::ResolvedImage,
        "padding" => PropertyKind::Padding,
        "numberArray" => PropertyKind::NumberArray,
        "colorArray" => PropertyKind::ColorArray,
        "projectionDefinition" => PropertyKind::ProjectionDefinition,
        "variableAnchorOffsetCollection" => PropertyKind::VariableAnchorOffsetCollection,
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
        (
            Value::Image(_)
            | Value::Formatted(_)
            | Value::Padding(_)
            | Value::NumberArray(_)
            | Value::ColorArray(_)
            | Value::ProjectionDefinition(_)
            | Value::VariableAnchorOffsetCollection(_),
            expected,
        ) => json_matches(&value.to_json(), expected),
        (value, expected) => value.to_json() == *expected,
    }
}

/// Whether two JSON documents agree, numbers within the precision the suite prints.
fn json_matches(actual: &Json, expected: &Json) -> bool {
    match (actual, expected) {
        (Json::Number(actual), Json::Number(expected)) => {
            let (actual, expected) = (
                actual.as_f64().unwrap_or(f64::NAN),
                expected.as_f64().unwrap_or(f64::NAN),
            );
            (actual - expected).abs() <= 1e-5 * expected.abs().max(1.0)
        }
        (Json::Array(actual), Json::Array(expected)) => {
            actual.len() == expected.len()
                && actual.iter().zip(expected).all(|(a, e)| json_matches(a, e))
        }
        (Json::Object(actual), Json::Object(expected)) => {
            actual.len() == expected.len()
                && actual
                    .iter()
                    .all(|(key, a)| expected.get(key).is_some_and(|e| json_matches(a, e)))
        }
        (actual, expected) => actual == expected,
    }
}

#[test]
fn the_vendored_suite_covers_every_supported_operator() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/expressions");
    for operator in [
        "match",
        "interpolate",
        "step",
        "legacy",
        "to-color",
        "let",
        "format",
        "image",
        "coalesce",
        "at",
        "split",
        "join",
        "error",
        "within",
        "distance",
        "collator",
        "number-format",
        "feature-state",
        "global-state",
        "accumulated",
    ] {
        assert!(
            root.join(operator).is_dir(),
            "{operator} cases are vendored"
        );
    }
    let _ = Type::Value;
}
