//! Number-valued paint expressions: legacy zoom and property functions plus the expression
//! operators a circle radius or line width commonly uses.
//!
//! Feature properties arrive as the strings the tile processors collect, so a numeric property
//! is parsed on use and a value that does not parse counts as missing.

use std::collections::HashMap;

use serde_json::Value;

/// Evaluates `expression` for a feature at `zoom`; `None` when the expression is outside the
/// supported subset or yields no number.
pub fn evaluate_number(
    expression: &Value,
    properties: &HashMap<String, String>,
    zoom: f64,
) -> Option<f64> {
    match expression {
        Value::Number(number) => number.as_f64(),
        Value::Object(function) => evaluate_function(function, properties, zoom),
        Value::Array(items) => evaluate_operator(items, properties, zoom),
        _ => None,
    }
}

fn property_number(properties: &HashMap<String, String>, key: &str) -> Option<f64> {
    properties.get(key)?.trim().parse().ok()
}

/// Legacy `{"stops": ..., "property": ..., "type": ...}` functions.
fn evaluate_function(
    function: &serde_json::Map<String, Value>,
    properties: &HashMap<String, String>,
    zoom: f64,
) -> Option<f64> {
    let kind = function.get("type").and_then(Value::as_str);
    let default = function.get("default").and_then(Value::as_f64);
    let property = function.get("property").and_then(Value::as_str);
    let base = function.get("base").and_then(Value::as_f64).unwrap_or(1.0);

    if let Some(key) = property {
        if kind == Some("identity") {
            return property_number(properties, key).or(default);
        }
        let stops = function.get("stops").and_then(Value::as_array)?;
        if stops
            .first()
            .and_then(Value::as_array)
            .and_then(|stop| stop.first())
            .is_some_and(Value::is_object)
        {
            return evaluate_composite(kind, base, key, stops, properties, zoom).or(default);
        }
        let result = match kind {
            Some("categorical") => {
                let value = properties.get(key)?;
                stops.iter().find_map(|stop| {
                    let stop = stop.as_array()?;
                    let label = match stop.first()? {
                        Value::String(label) => label.clone(),
                        Value::Number(number) => number.to_string(),
                        Value::Bool(flag) => flag.to_string(),
                        _ => return None,
                    };
                    (&label == value).then(|| stop.get(1)?.as_f64())?
                })
            }
            Some("interval") => step(property_number(properties, key)?, &numeric_stops(stops)?),
            _ => interpolate(
                property_number(properties, key)?,
                base,
                &numeric_stops(stops)?,
            ),
        };
        return result.or(default);
    }

    let stops = numeric_stops(function.get("stops").and_then(Value::as_array)?)?;
    match kind {
        Some("interval") => step(zoom, &stops),
        _ => interpolate(zoom, base, &stops),
    }
}

/// Composite functions: stops keyed by `{zoom, value}` are grouped per zoom, evaluated as a
/// property function for each zoom, and the two zoom groups around `zoom` are blended.
fn evaluate_composite(
    kind: Option<&str>,
    base: f64,
    key: &str,
    stops: &[Value],
    properties: &HashMap<String, String>,
    zoom: f64,
) -> Option<f64> {
    let mut groups: Vec<(f64, Vec<(Value, f64)>)> = Vec::new();
    for stop in stops {
        let stop = stop.as_array()?;
        let input = stop.first()?.as_object()?;
        let stop_zoom = input.get("zoom")?.as_f64()?;
        let value = input.get("value")?.clone();
        let output = stop.get(1)?.as_f64()?;
        match groups
            .iter_mut()
            .find(|(group_zoom, _)| *group_zoom == stop_zoom)
        {
            Some((_, group)) => group.push((value, output)),
            None => groups.push((stop_zoom, vec![(value, output)])),
        }
    }
    groups.sort_by(|(a, _), (b, _)| a.total_cmp(b));
    let evaluate_group = |group: &[(Value, f64)]| -> Option<f64> {
        match kind {
            Some("categorical") => {
                let value = properties.get(key)?;
                group.iter().find_map(|(label, output)| {
                    let label = match label {
                        Value::String(label) => label.clone(),
                        Value::Number(number) => number.to_string(),
                        Value::Bool(flag) => flag.to_string(),
                        _ => return None,
                    };
                    (&label == value).then_some(*output)
                })
            }
            Some("interval") => {
                let numeric: Vec<(f64, f64)> = group
                    .iter()
                    .filter_map(|(value, output)| Some((value.as_f64()?, *output)))
                    .collect();
                step(property_number(properties, key)?, &numeric)
            }
            _ => {
                let numeric: Vec<(f64, f64)> = group
                    .iter()
                    .filter_map(|(value, output)| Some((value.as_f64()?, *output)))
                    .collect();
                interpolate(property_number(properties, key)?, base, &numeric)
            }
        }
    };
    let per_zoom: Vec<(f64, f64)> = groups
        .iter()
        .filter_map(|(group_zoom, group)| Some((*group_zoom, evaluate_group(group)?)))
        .collect();
    match kind {
        Some("interval") => step(zoom, &per_zoom),
        _ => interpolate(zoom, base, &per_zoom),
    }
}

fn numeric_stops(stops: &[Value]) -> Option<Vec<(f64, f64)>> {
    stops
        .iter()
        .map(|stop| {
            let stop = stop.as_array()?;
            Some((stop.first()?.as_f64()?, stop.get(1)?.as_f64()?))
        })
        .collect()
}

/// The output of the last stop at or below `input`, as GL JS `step` and interval functions.
fn step(input: f64, stops: &[(f64, f64)]) -> Option<f64> {
    let (first, _) = stops.first()?;
    if input < *first {
        return Some(stops[0].1);
    }
    stops
        .iter()
        .take_while(|(stop, _)| *stop <= input)
        .last()
        .map(|(_, output)| *output)
}

/// Exponential interpolation between stops; a base of one is linear.
fn interpolate(input: f64, base: f64, stops: &[(f64, f64)]) -> Option<f64> {
    let (first, last) = (stops.first()?, stops.last()?);
    if input <= first.0 {
        return Some(first.1);
    }
    if input >= last.0 {
        return Some(last.1);
    }
    let upper = stops.iter().position(|(stop, _)| *stop > input)?;
    let (lower_stop, lower_output) = stops[upper - 1];
    let (upper_stop, upper_output) = stops[upper];
    let range = upper_stop - lower_stop;
    let progress = input - lower_stop;
    let t = if range == 0.0 {
        0.0
    } else if (base - 1.0).abs() < f64::EPSILON {
        progress / range
    } else {
        (base.powf(progress) - 1.0) / (base.powf(range) - 1.0)
    };
    Some(lower_output + t * (upper_output - lower_output))
}

fn evaluate_operator(
    items: &[Value],
    properties: &HashMap<String, String>,
    zoom: f64,
) -> Option<f64> {
    let operator = items.first()?.as_str()?;
    let number = |value: &Value| evaluate_number(value, properties, zoom);
    match operator {
        "get" => property_number(properties, items.get(1)?.as_str()?),
        "zoom" => Some(zoom),
        "literal" | "number" | "to-number" => number(items.get(1)?),
        "interpolate" => {
            let kind = items.get(1)?.as_array()?;
            let base = match kind.first()?.as_str()? {
                "exponential" => kind.get(1)?.as_f64()?,
                _ => 1.0,
            };
            let input = number(items.get(2)?)?;
            let stops = items[3..]
                .chunks(2)
                .map(|pair| Some((pair.first()?.as_f64()?, number(pair.get(1)?)?)))
                .collect::<Option<Vec<_>>>()?;
            interpolate(input, base, &stops)
        }
        "step" => {
            let input = number(items.get(1)?)?;
            let first = number(items.get(2)?)?;
            let mut stops = vec![(f64::NEG_INFINITY, first)];
            for pair in items[3..].chunks(2) {
                stops.push((pair.first()?.as_f64()?, number(pair.get(1)?)?));
            }
            step(input, &stops)
        }
        "match" => {
            let input = match items.get(1)? {
                Value::Array(inner) if inner.first().and_then(Value::as_str) == Some("get") => {
                    properties.get(inner.get(1)?.as_str()?)?.clone()
                }
                Value::String(text) => text.clone(),
                Value::Number(value) => value.to_string(),
                _ => return None,
            };
            let rest = &items[2..];
            let fallback = rest.last()?;
            for pair in rest[..rest.len().saturating_sub(1)].chunks(2) {
                let labels = match pair.first()? {
                    Value::Array(labels) => labels.clone(),
                    label => vec![label.clone()],
                };
                let matches = labels.iter().any(|label| match label {
                    Value::String(label) => *label == input,
                    Value::Number(label) => label.to_string() == input,
                    _ => false,
                });
                if matches {
                    return number(pair.get(1)?);
                }
            }
            number(fallback)
        }
        "coalesce" => items[1..].iter().find_map(number),
        "+" => items[1..].iter().map(number).sum::<Option<f64>>(),
        "*" => items[1..].iter().map(number).product::<Option<f64>>(),
        "-" => {
            let first = number(items.get(1)?)?;
            match items.get(2) {
                Some(second) => Some(first - number(second)?),
                None => Some(-first),
            }
        }
        "/" => Some(number(items.get(1)?)? / number(items.get(2)?)?),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::panic)]

    use std::collections::HashMap;

    use serde_json::json;

    use super::evaluate_number;

    fn properties() -> HashMap<String, String> {
        HashMap::from([
            ("width".to_string(), "7".to_string()),
            ("kind".to_string(), "airport".to_string()),
            ("mag".to_string(), "2.5".to_string()),
        ])
    }

    fn eval(expression: serde_json::Value, zoom: f64) -> Option<f64> {
        evaluate_number(&expression, &properties(), zoom)
    }

    #[test]
    fn legacy_functions() {
        assert_eq!(eval(json!(4), 0.0), Some(4.0));
        assert_eq!(
            eval(json!({"type": "identity", "property": "width"}), 0.0),
            Some(7.0)
        );
        assert_eq!(
            eval(
                json!({"type": "identity", "property": "missing", "default": 2}),
                0.0
            ),
            Some(2.0)
        );
        assert_eq!(
            eval(json!({"property": "mag", "stops": [[0, 0], [5, 10]]}), 0.0),
            Some(5.0)
        );
        assert_eq!(
            eval(
                json!({"property": "kind", "type": "categorical", "stops": [["airport", 9], ["vor", 3]]}),
                0.0
            ),
            Some(9.0)
        );
        assert_eq!(
            eval(
                json!({"property": "mag", "type": "interval", "stops": [[0, 1], [2, 2], [3, 3]]}),
                0.0
            ),
            Some(2.0)
        );
        assert_eq!(eval(json!({"stops": [[2, 1], [4, 3]]}), 3.0), Some(2.0));
        assert_eq!(eval(json!({"stops": [[2, 1], [4, 3]]}), 9.0), Some(3.0));
        assert_eq!(
            eval(json!({"type": "interval", "stops": [[2, 1], [4, 3]]}), 3.0),
            Some(1.0)
        );
        let exponential = eval(json!({"base": 2, "stops": [[0, 0], [2, 3]]}), 1.0).expect("value");
        assert!(
            (exponential - 1.0).abs() < 1e-9,
            "(2^1 - 1) / (2^2 - 1) * 3 = 1"
        );
    }

    #[test]
    fn expression_operators() {
        assert_eq!(eval(json!(["get", "width"]), 0.0), Some(7.0));
        assert_eq!(eval(json!(["get", "kind"]), 0.0), None);
        assert_eq!(eval(json!(["zoom"]), 6.5), Some(6.5));
        assert_eq!(
            eval(
                json!(["interpolate", ["linear"], ["zoom"], 0, 2, 10, 12]),
                5.0
            ),
            Some(7.0)
        );
        assert_eq!(
            eval(
                json!([
                    "interpolate",
                    ["exponential", 2],
                    ["get", "mag"],
                    0,
                    0,
                    5,
                    31
                ]),
                0.0
            ),
            Some(31.0 * (2.0_f64.powf(2.5) - 1.0) / (2.0_f64.powf(5.0) - 1.0))
        );
        assert_eq!(
            eval(json!(["step", ["get", "mag"], 1, 2, 5, 3, 9]), 0.0),
            Some(5.0)
        );
        assert_eq!(eval(json!(["step", ["zoom"], 1, 2, 5]), 1.0), Some(1.0));
        assert_eq!(
            eval(
                json!(["match", ["get", "kind"], ["vor", "airport"], 8, 2]),
                0.0
            ),
            Some(8.0)
        );
        assert_eq!(
            eval(json!(["match", ["get", "kind"], "vor", 8, 2]), 0.0),
            Some(2.0)
        );
        assert_eq!(
            eval(json!(["coalesce", ["get", "missing"], 4]), 0.0),
            Some(4.0)
        );
        assert_eq!(eval(json!(["*", ["get", "width"], 2]), 0.0), Some(14.0));
        assert_eq!(eval(json!(["-", 10, ["get", "width"]]), 0.0), Some(3.0));
        assert_eq!(eval(json!(["/", ["get", "width"], 2]), 0.0), Some(3.5));
        assert_eq!(eval(json!(["+", 1, 2, 3]), 0.0), Some(6.0));
        assert_eq!(eval(json!(["unknown", 1]), 0.0), None);
    }

    #[test]
    fn composite_zoom_and_property_functions() {
        let function = json!({"property": "mag", "stops": [
            [{"zoom": 0, "value": 0}, 0.0], [{"zoom": 0, "value": 5}, 10.0],
            [{"zoom": 2, "value": 0}, 20.0], [{"zoom": 2, "value": 5}, 30.0]
        ]});
        assert_eq!(evaluate_number(&function, &properties(), 0.0), Some(5.0));
        assert_eq!(evaluate_number(&function, &properties(), 2.0), Some(25.0));
        assert_eq!(evaluate_number(&function, &properties(), 1.0), Some(15.0));
        assert_eq!(evaluate_number(&function, &HashMap::new(), 1.0), None);

        let categorical = json!({"property": "kind", "type": "categorical", "stops": [
            [{"zoom": 0, "value": "airport"}, 1.0], [{"zoom": 4, "value": "airport"}, 3.0]
        ]});
        assert_eq!(evaluate_number(&categorical, &properties(), 2.0), Some(2.0));
    }
}
