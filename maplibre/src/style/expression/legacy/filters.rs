//! Lowering of legacy filters into boolean expressions, as GL JS `convertFilter` does.

use serde_json::{json, Map, Value as Json};

use super::push;

/// Whether a filter is written in expression syntax rather than the legacy one.
pub fn is_expression_filter(filter: &Json) -> bool {
    let Json::Array(items) = filter else {
        return matches!(filter, Json::Bool(_));
    };
    let Some(operator) = items.first().and_then(Json::as_str) else {
        return false;
    };
    match operator {
        "has" => items.len() >= 2 && !matches!(items[1].as_str(), Some("$id") | Some("$type")),
        "in" => items.len() >= 3 && (!items[1].is_string() || items[2].is_array()),
        "!in" | "!has" | "none" => false,
        "==" | "!=" | ">" | ">=" | "<" | "<=" => {
            items.len() != 3 || items[1].is_array() || items[2].is_array()
        }
        "any" | "all" => items[1..]
            .iter()
            .all(|child| is_expression_filter(child) || child.is_boolean()),
        _ => true,
    }
}

/// Lowers a legacy filter into an expression, as GL JS `convertFilter` does; an expression
/// filter is returned as it is.
pub fn convert_filter(filter: &Json) -> Json {
    convert_filter_with(filter, &mut Map::new())
}

fn convert_filter_with(filter: &Json, expected_types: &mut Map<String, Json>) -> Json {
    if is_expression_filter(filter) {
        return filter.clone();
    }
    let Json::Array(items) = filter else {
        return json!(true);
    };
    let operator = items.first().and_then(Json::as_str).unwrap_or("");
    if items.len() <= 1 {
        return json!(operator != "any");
    }
    match operator {
        "==" | "!=" | "<" | ">" | "<=" | ">=" => {
            let property = items.get(1).and_then(Json::as_str).unwrap_or("");
            let value = items.get(2).cloned().unwrap_or(Json::Null);
            convert_comparison_op(property, &value, operator, Some(expected_types))
        }
        "any" => {
            let children: Vec<Json> = items[1..]
                .iter()
                .map(|condition| {
                    let mut types = Map::new();
                    let child = convert_filter_with(condition, &mut types);
                    match runtime_type_checks(&types) {
                        Json::Bool(true) => child,
                        checks => json!(["case", checks, child, false]),
                    }
                })
                .collect();
            let mut expression = json!(["any"]);
            for child in children {
                push(&mut expression, child);
            }
            expression
        }
        "all" => {
            let children: Vec<Json> = items[1..]
                .iter()
                .map(|condition| convert_filter_with(condition, expected_types))
                .collect();
            if children.len() == 1 {
                return children[0].clone();
            }
            let mut expression = json!(["all"]);
            for child in children {
                push(&mut expression, child);
            }
            expression
        }
        "none" => {
            let mut any = json!(["any"]);
            for condition in &items[1..] {
                push(&mut any, condition.clone());
            }
            json!(["!", convert_filter_with(&any, &mut Map::new())])
        }
        "in" | "!in" => {
            let property = items.get(1).and_then(Json::as_str).unwrap_or("");
            convert_in_op(property, &items[2..], operator == "!in")
        }
        "has" => convert_has_op(items.get(1).and_then(Json::as_str).unwrap_or("")),
        "!has" => json!([
            "!",
            convert_has_op(items.get(1).and_then(Json::as_str).unwrap_or(""))
        ]),
        _ => json!(true),
    }
}

fn runtime_type_checks(expected_types: &Map<String, Json>) -> Json {
    let conditions: Vec<Json> = expected_types
        .iter()
        .map(|(property, kind)| {
            let get = if property == "$id" {
                json!(["id"])
            } else {
                json!(["get", property])
            };
            json!(["==", ["typeof", get], kind])
        })
        .collect();
    match conditions.len() {
        0 => json!(true),
        1 => conditions[0].clone(),
        _ => {
            let mut all = json!(["all"]);
            for condition in conditions {
                push(&mut all, condition);
            }
            all
        }
    }
}

fn property_getter(property: &str) -> Json {
    match property {
        "$type" => json!(["geometry-type"]),
        "$id" => json!(["id"]),
        _ => json!(["get", property]),
    }
}

fn convert_comparison_op(
    property: &str,
    value: &Json,
    operator: &str,
    expected_types: Option<&mut Map<String, Json>>,
) -> Json {
    if property == "$type" {
        return json!([operator, ["geometry-type"], value]);
    }
    let get = property_getter(property);
    if let Some(expected_types) = expected_types {
        if !value.is_null() {
            let kind = match value {
                Json::Bool(_) => "boolean",
                Json::Number(_) => "number",
                Json::String(_) => "string",
                _ => "object",
            };
            expected_types.insert(property.to_string(), json!(kind));
        }
    }
    if operator == "==" && property != "$id" && value.is_null() {
        // A missing property is not null for legacy filters.
        return json!(["all", ["has", property], ["==", get, null]]);
    }
    if operator == "!=" && property != "$id" && value.is_null() {
        return json!(["any", ["!", ["has", property]], ["!=", get, null]]);
    }
    json!([operator, get, value])
}

fn convert_in_op(property: &str, values: &[Json], negate: bool) -> Json {
    if values.is_empty() {
        return json!(negate);
    }
    let get = property_getter(property);
    let uniform = values
        .iter()
        .all(|value| std::mem::discriminant(value) == std::mem::discriminant(&values[0]));
    if uniform && (values[0].is_string() || values[0].is_number()) {
        let mut unique: Vec<Json> = Vec::new();
        for value in values {
            if !unique.contains(value) {
                unique.push(value.clone());
            }
        }
        return json!(["match", get, unique, !negate, negate]);
    }
    let mut expression = json!([if negate { "all" } else { "any" }]);
    for value in values {
        push(
            &mut expression,
            json!([if negate { "!=" } else { "==" }, get, value]),
        );
    }
    expression
}

fn convert_has_op(property: &str) -> Json {
    match property {
        "$type" => json!(true),
        "$id" => json!(["!=", ["id"], null]),
        _ => json!(["has", property]),
    }
}
