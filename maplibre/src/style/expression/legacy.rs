//! Lowering of legacy style syntax, function objects and filters, into expressions, following
//! the GL JS `convertFunction` and `convertFilter` rules so both share one evaluator.

use serde_json::{json, Map, Value as Json};

use super::value::Type;

mod filters;

pub use filters::{convert_filter, is_expression_filter};

/// The kind of value a style property holds, which decides how legacy syntax is lowered.
#[derive(Clone, Debug, PartialEq)]
pub enum PropertyKind {
    /// Any value; what a property without a specification accepts.
    Value,
    /// A number.
    Number,
    /// A string.
    String,
    /// A boolean.
    Boolean,
    /// A colour.
    Color,
    /// One of a fixed set of strings.
    Enum(Vec<String>),
    /// An array of one item kind, with a fixed length when given.
    Array {
        /// Kind of every item.
        item: Box<PropertyKind>,
        /// Number of items.
        length: Option<usize>,
    },
}

impl PropertyKind {
    /// The expression type a property of this kind must produce.
    pub fn expected_type(&self) -> Type {
        match self {
            Self::Value => Type::Value,
            Self::Number => Type::Number,
            Self::String | Self::Enum(_) => Type::String,
            Self::Boolean => Type::Boolean,
            Self::Color => Type::Color,
            Self::Array { item, length } => Type::array(item.expected_type(), *length),
        }
    }

    fn assertion_name(&self) -> &'static str {
        match self {
            Self::Value => "coalesce",
            Self::Number => "number",
            Self::String | Self::Enum(_) => "string",
            Self::Boolean => "boolean",
            Self::Color => "to-color",
            Self::Array { .. } => "array",
        }
    }
}

/// What the specification says about a property, as far as legacy syntax needs it.
#[derive(Clone, Debug, PartialEq)]
pub struct LegacyPropertySpec {
    /// Kind of value the property holds.
    pub kind: PropertyKind,
    /// Whether the property interpolates between stops; otherwise stops are steps.
    pub interpolated: bool,
    /// The specification's default, used when a function names none.
    pub default: Option<Json>,
    /// Whether string values may hold `{token}` references to feature properties.
    pub tokens: bool,
}

impl LegacyPropertySpec {
    /// A spec for an interpolated property of `kind` with no default.
    pub fn interpolated(kind: PropertyKind) -> Self {
        Self {
            kind,
            interpolated: true,
            default: None,
            tokens: false,
        }
    }

    /// A spec for a stepped property of `kind` with no default.
    pub fn stepped(kind: PropertyKind) -> Self {
        Self {
            kind,
            interpolated: false,
            default: None,
            tokens: false,
        }
    }

    /// The expression type the property must produce.
    pub fn expected_type(&self) -> Type {
        self.kind.expected_type()
    }
}

fn literal(value: &Json) -> Json {
    match value {
        Json::Array(_) | Json::Object(_) => json!(["literal", value]),
        other => other.clone(),
    }
}

/// Lowers a legacy function object into an expression, as GL JS `convertFunction` does.
pub fn convert_function(parameters: &Map<String, Json>, spec: &LegacyPropertySpec) -> Json {
    let Some(stops) = parameters.get("stops").and_then(Json::as_array) else {
        return convert_identity_function(parameters, spec);
    };
    let zoom_and_feature = stops
        .first()
        .and_then(|stop| stop.get(0))
        .is_some_and(Json::is_object);
    let feature_dependent = zoom_and_feature || parameters.contains_key("property");
    let zoom_dependent = zoom_and_feature || !feature_dependent;
    let stops: Vec<(Json, Json)> = stops
        .iter()
        .map(|stop| {
            let input = stop.get(0).cloned().unwrap_or(Json::Null);
            let output = stop.get(1).cloned().unwrap_or(Json::Null);
            let output = match &output {
                Json::String(text) if !feature_dependent && spec.tokens => {
                    convert_token_string(text)
                }
                other => literal(other),
            };
            (input, output)
        })
        .collect();
    if zoom_and_feature {
        convert_zoom_and_property_function(parameters, spec, &stops)
    } else if zoom_dependent {
        convert_zoom_function(parameters, spec, &stops)
    } else {
        convert_property_function(parameters, spec, &stops)
    }
}

fn convert_identity_function(parameters: &Map<String, Json>, spec: &LegacyPropertySpec) -> Json {
    let get = json!([
        "get",
        parameters.get("property").cloned().unwrap_or(Json::Null)
    ]);
    match parameters.get("default") {
        None => match spec.kind {
            PropertyKind::String => json!(["string", get]),
            _ => get,
        },
        Some(default) => match &spec.kind {
            PropertyKind::Enum(values) => json!(["match", get, values, get, default]),
            PropertyKind::Array { item, length } => json!([
                "array",
                item.assertion_name(),
                length.map_or(Json::Null, |length| json!(length)),
                get,
                literal(default)
            ]),
            kind => json!([kind.assertion_name(), get, literal(default)]),
        },
    }
}

fn interpolate_operator(parameters: &Map<String, Json>) -> &'static str {
    match parameters.get("colorSpace").and_then(Json::as_str) {
        Some("hcl") => "interpolate-hcl",
        Some("lab") => "interpolate-lab",
        _ => "interpolate",
    }
}

fn function_type<'a>(parameters: &'a Map<String, Json>, spec: &LegacyPropertySpec) -> &'a str {
    match parameters.get("type").and_then(Json::as_str) {
        Some(kind) => kind,
        None if spec.interpolated => "exponential",
        None => "interval",
    }
}

fn convert_zoom_and_property_function(
    parameters: &Map<String, Json>,
    spec: &LegacyPropertySpec,
    stops: &[(Json, Json)],
) -> Json {
    let mut zoom_stops: Vec<(f64, Vec<(Json, Json)>)> = Vec::new();
    for (input, output) in stops {
        let zoom = input.get("zoom").and_then(Json::as_f64).unwrap_or(0.0);
        let value = input.get("value").cloned().unwrap_or(Json::Null);
        match zoom_stops.iter_mut().find(|(stop, _)| *stop == zoom) {
            Some((_, group)) => group.push((value, output.clone())),
            None => zoom_stops.push((zoom, vec![(value, output.clone())])),
        }
    }
    let mut feature_parameters = Map::new();
    for key in ["type", "property", "default"] {
        if let Some(value) = parameters.get(key) {
            feature_parameters.insert(key.to_string(), value.clone());
        }
    }
    let exponential = function_type(&Map::new(), spec) == "exponential";
    let mut expression = if exponential {
        json!([interpolate_operator(parameters), ["linear"], ["zoom"]])
    } else {
        json!(["step", ["zoom"]])
    };
    for (zoom, group) in &zoom_stops {
        let output = convert_property_function(&feature_parameters, spec, group);
        append_stop_pair(&mut expression, json!(zoom), output, !exponential);
    }
    if !exponential {
        fixup_degenerate_step_curve(&mut expression);
    }
    expression
}

fn fallback(parameters: &Map<String, Json>, spec: &LegacyPropertySpec) -> Json {
    parameters
        .get("default")
        .or(spec.default.as_ref())
        .map_or(Json::Null, literal)
}

fn convert_property_function(
    parameters: &Map<String, Json>,
    spec: &LegacyPropertySpec,
    stops: &[(Json, Json)],
) -> Json {
    let kind = function_type(parameters, spec);
    let get = json!([
        "get",
        parameters.get("property").cloned().unwrap_or(Json::Null)
    ]);
    match kind {
        "categorical" if stops.first().is_some_and(|(input, _)| input.is_boolean()) => {
            let mut expression = json!(["case"]);
            for (input, output) in stops {
                push(&mut expression, json!(["==", get, input]));
                push(&mut expression, output.clone());
            }
            push(&mut expression, fallback(parameters, spec));
            expression
        }
        "categorical" => {
            let mut expression = json!(["match", get]);
            for (input, output) in stops {
                append_stop_pair(&mut expression, input.clone(), output.clone(), false);
            }
            push(&mut expression, fallback(parameters, spec));
            expression
        }
        "interval" => {
            let mut expression = json!(["step", ["number", get]]);
            for (input, output) in stops {
                append_stop_pair(&mut expression, input.clone(), output.clone(), true);
            }
            fixup_degenerate_step_curve(&mut expression);
            guard_default(parameters, &get, expression)
        }
        _ => {
            let base = parameters.get("base").and_then(Json::as_f64).unwrap_or(1.0);
            let curve = if base == 1.0 {
                json!(["linear"])
            } else {
                json!(["exponential", base])
            };
            let mut expression = json!([interpolate_operator(parameters), curve, ["number", get]]);
            for (input, output) in stops {
                append_stop_pair(&mut expression, input.clone(), output.clone(), false);
            }
            guard_default(parameters, &get, expression)
        }
    }
}

/// Wraps a numeric ramp so a feature without a numeric value takes the default.
fn guard_default(parameters: &Map<String, Json>, get: &Json, expression: Json) -> Json {
    match parameters.get("default") {
        None => expression,
        Some(default) => json!([
            "case",
            ["==", ["typeof", get], "number"],
            expression,
            literal(default)
        ]),
    }
}

fn convert_zoom_function(
    parameters: &Map<String, Json>,
    spec: &LegacyPropertySpec,
    stops: &[(Json, Json)],
) -> Json {
    let kind = function_type(parameters, spec);
    let is_step = kind == "interval";
    let mut expression = if is_step {
        json!(["step", ["zoom"]])
    } else {
        let base = parameters.get("base").and_then(Json::as_f64).unwrap_or(1.0);
        let curve = if base == 1.0 {
            json!(["linear"])
        } else {
            json!(["exponential", base])
        };
        json!([interpolate_operator(parameters), curve, ["zoom"]])
    };
    for (input, output) in stops {
        append_stop_pair(&mut expression, input.clone(), output.clone(), is_step);
    }
    fixup_degenerate_step_curve(&mut expression);
    expression
}

pub(super) fn push(expression: &mut Json, value: Json) {
    if let Json::Array(items) = expression {
        items.push(value);
    }
}

fn fixup_degenerate_step_curve(expression: &mut Json) {
    if let Json::Array(items) = expression {
        if items.first().and_then(Json::as_str) == Some("step") && items.len() == 3 {
            let output = items[2].clone();
            items.push(json!(0));
            items.push(output);
        }
    }
}

fn append_stop_pair(curve: &mut Json, input: Json, output: Json, is_step: bool) {
    let Json::Array(items) = curve else {
        return;
    };
    if items.len() > 3 && items[items.len() - 2] == input {
        return;
    }
    if !(is_step && items.len() == 2) {
        items.push(input);
    }
    items.push(output);
}

/// Turns a `{token}` string into a `concat` of literals and property reads.
pub fn convert_token_string(text: &str) -> Json {
    let mut parts: Vec<Json> = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}') else {
            break;
        };
        let name = &rest[open + 1..open + close];
        if name.is_empty() || name.contains('{') {
            break;
        }
        if open > 0 {
            parts.push(Json::String(rest[..open].to_string()));
        }
        parts.push(json!(["get", name]));
        rest = &rest[open + close + 1..];
    }
    if parts.is_empty() {
        return Json::String(text.to_string());
    }
    if !rest.is_empty() {
        parts.push(Json::String(rest.to_string()));
    } else if parts.len() == 1 {
        return json!(["to-string", parts[0]]);
    }
    let mut expression = json!(["concat"]);
    for part in parts {
        push(&mut expression, part);
    }
    expression
}
