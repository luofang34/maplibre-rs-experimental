//! Feature filters of style layers: the legacy filter syntax and the expression subset GL JS
//! accepts in a layer's `filter`, parsed once into a typed tree and evaluated per feature.
//!
//! Legacy filters name a property directly (`["==", "level", "low"]`) while expression filters
//! read it through an operand (`["==", ["get", "level"], "low"]`). Both forms compare typed
//! values, so a number never equals its string spelling and a missing property is unequal to
//! everything. An operator outside the supported subset is a parse error rather than a filter
//! that quietly admits or drops features.

use std::collections::HashMap;

use serde_json::Value;
use thiserror::Error;

use self::parse::{is_expression_filter, kind_name, parse_expression, parse_legacy};

mod parse;

/// Why a filter cannot be evaluated by this renderer.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum FilterError {
    /// The operator is not part of the supported subset.
    #[error("unsupported filter operator `{operator}`")]
    UnsupportedOperator {
        /// The operator as written in the style.
        operator: String,
    },
    /// The operator is known but its arguments have the wrong shape.
    #[error("filter operator `{operator}` expects {expected}")]
    Malformed {
        /// The operator whose arguments are wrong.
        operator: String,
        /// The argument shape the operator takes.
        expected: &'static str,
    },
    /// The filter is neither an array nor a boolean.
    #[error("filter must be an array or a boolean, found {found}")]
    NotAFilter {
        /// The kind of JSON value found instead.
        found: String,
    },
}

/// The geometry kind a filter can test with `$type` or `["geometry-type"]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeometryType {
    /// A geometry kind neither MVT nor GeoJSON names.
    Unknown,
    /// A point or multi-point.
    Point,
    /// A line string or multi-line string.
    LineString,
    /// A polygon or multi-polygon.
    Polygon,
}

impl GeometryType {
    /// Maps an MVT `GeomType` number.
    pub fn from_mvt(kind: i32) -> Self {
        match kind {
            1 => Self::Point,
            2 => Self::LineString,
            3 => Self::Polygon,
            _ => Self::Unknown,
        }
    }

    /// Maps a GeoJSON geometry type name; multi-geometries share their member kind.
    pub fn from_geojson(name: &str) -> Self {
        match name {
            "Point" | "MultiPoint" => Self::Point,
            "LineString" | "MultiLineString" => Self::LineString,
            "Polygon" | "MultiPolygon" => Self::Polygon,
            _ => Self::Unknown,
        }
    }

    /// The name `["geometry-type"]` evaluates to.
    pub fn name(self) -> &'static str {
        match self {
            Self::Unknown => "Unknown",
            Self::Point => "Point",
            Self::LineString => "LineString",
            Self::Polygon => "Polygon",
        }
    }
}

/// What a filter can observe about one feature.
pub struct FeatureContext<'a> {
    /// The feature's typed properties.
    pub properties: &'a HashMap<String, Value>,
    /// The feature's geometry kind.
    pub geometry_type: GeometryType,
    /// The feature's id, when the source assigned one.
    pub id: Option<Value>,
    /// Zoom of the tile being processed, which is what GL JS hands to filters.
    pub zoom: f64,
}

/// The comparison operators a filter can apply to two operands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Comparison {
    /// `==`
    Equal,
    /// `!=`
    NotEqual,
    /// `<`
    Less,
    /// `<=`
    LessEqual,
    /// `>`
    Greater,
    /// `>=`
    GreaterEqual,
}

impl Comparison {
    pub(super) fn parse(operator: &str) -> Option<Self> {
        Some(match operator {
            "==" => Self::Equal,
            "!=" => Self::NotEqual,
            "<" => Self::Less,
            "<=" => Self::LessEqual,
            ">" => Self::Greater,
            ">=" => Self::GreaterEqual,
            _ => return None,
        })
    }
}

/// A value-producing sub-expression of a filter.
#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    /// A constant value.
    Literal(Value),
    /// A feature property by name.
    Get(String),
    /// The feature's geometry kind name.
    GeometryType,
    /// The feature's id.
    Id,
    /// The zoom the filter is evaluated at.
    Zoom,
    /// The inner operand converted to a number.
    ToNumber(Box<Operand>),
    /// The inner operand converted to a string.
    ToString(Box<Operand>),
}

/// A parsed filter.
#[derive(Debug, Clone, PartialEq)]
pub enum Filter {
    /// Passes or rejects every feature.
    Literal(bool),
    /// Passes when every nested filter passes.
    All(Vec<Filter>),
    /// Passes when any nested filter passes.
    Any(Vec<Filter>),
    /// Passes when the nested filter rejects.
    Not(Box<Filter>),
    /// Compares two operands.
    Compare {
        /// The comparison to apply.
        operator: Comparison,
        /// The left-hand operand.
        left: Operand,
        /// The right-hand operand.
        right: Operand,
    },
    /// Passes when the feature has the property.
    Has(String),
    /// Passes when the feature has an id.
    HasId,
    /// Passes when the haystack contains the needle.
    In {
        /// The value looked for.
        needle: Operand,
        /// An array to search, or a string to search for a substring.
        haystack: Operand,
    },
    /// Selects the filter whose labels contain the input.
    Match {
        /// The value matched against the labels.
        input: Operand,
        /// Labels and the filter each label set selects.
        cases: Vec<(Vec<Value>, Filter)>,
        /// The filter when no label matches.
        fallback: Box<Filter>,
    },
    /// Selects the output of the first passing condition.
    Case {
        /// Condition and output pairs, tried in order.
        branches: Vec<(Filter, Filter)>,
        /// The output when no condition passes.
        fallback: Box<Filter>,
    },
}

impl Filter {
    /// Parses a layer's `filter` value; `null` and an empty array pass every feature.
    pub fn parse(value: &Value) -> Result<Self, FilterError> {
        match value {
            Value::Null => Ok(Self::Literal(true)),
            Value::Bool(pass) => Ok(Self::Literal(*pass)),
            Value::Array(items) if items.is_empty() => Ok(Self::Literal(true)),
            Value::Array(items) if is_expression_filter(items) => parse_expression(items),
            Value::Array(items) => parse_legacy(items),
            other => Err(FilterError::NotAFilter {
                found: kind_name(other).to_string(),
            }),
        }
    }

    /// Whether the feature passes the filter.
    pub fn evaluate(&self, feature: &FeatureContext) -> bool {
        match self {
            Self::Literal(pass) => *pass,
            Self::All(filters) => filters.iter().all(|filter| filter.evaluate(feature)),
            Self::Any(filters) => filters.iter().any(|filter| filter.evaluate(feature)),
            Self::Not(filter) => !filter.evaluate(feature),
            Self::Compare {
                operator,
                left,
                right,
            } => compare(*operator, &left.evaluate(feature), &right.evaluate(feature)),
            Self::Has(key) => feature.properties.contains_key(key),
            Self::HasId => feature.id.is_some(),
            Self::In { needle, haystack } => {
                contains(&haystack.evaluate(feature), &needle.evaluate(feature))
            }
            Self::Match {
                input,
                cases,
                fallback,
            } => {
                let input = input.evaluate(feature);
                cases
                    .iter()
                    .find(|(labels, _)| labels.iter().any(|label| values_equal(label, &input)))
                    .map_or_else(
                        || fallback.evaluate(feature),
                        |(_, output)| output.evaluate(feature),
                    )
            }
            Self::Case { branches, fallback } => branches
                .iter()
                .find(|(condition, _)| condition.evaluate(feature))
                .map_or_else(
                    || fallback.evaluate(feature),
                    |(_, output)| output.evaluate(feature),
                ),
        }
    }
}

impl Operand {
    fn evaluate(&self, feature: &FeatureContext) -> Value {
        match self {
            Self::Literal(value) => value.clone(),
            Self::Get(key) => feature.properties.get(key).cloned().unwrap_or(Value::Null),
            Self::GeometryType => Value::String(feature.geometry_type.name().to_string()),
            Self::Id => feature.id.clone().unwrap_or(Value::Null),
            Self::Zoom => {
                serde_json::Number::from_f64(feature.zoom).map_or(Value::Null, Value::Number)
            }
            Self::ToNumber(inner) => match inner.evaluate(feature) {
                Value::Number(number) => Value::Number(number),
                Value::Bool(flag) => Value::from(u8::from(flag)),
                Value::String(text) => text
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .and_then(serde_json::Number::from_f64)
                    .map_or(Value::Null, Value::Number),
                _ => Value::Null,
            },
            Self::ToString(inner) => match inner.evaluate(feature) {
                Value::String(text) => Value::String(text),
                Value::Number(number) => Value::String(
                    number
                        .as_f64()
                        .map_or_else(|| number.to_string(), |value| value.to_string()),
                ),
                Value::Bool(flag) => Value::String(flag.to_string()),
                Value::Null => Value::String(String::new()),
                other => Value::String(other.to_string()),
            },
        }
    }
}

/// Typed equality: numbers compare as `f64`, and values of different kinds are never equal.
pub fn values_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => left.as_f64() == right.as_f64(),
        (Value::String(left), Value::String(right)) => left == right,
        (Value::Bool(left), Value::Bool(right)) => left == right,
        (Value::Null, Value::Null) => true,
        _ => false,
    }
}

fn compare(operator: Comparison, left: &Value, right: &Value) -> bool {
    let ordering = match (left, right) {
        (Value::Number(left), Value::Number(right)) => left.as_f64().partial_cmp(&right.as_f64()),
        (Value::String(left), Value::String(right)) => Some(left.cmp(right)),
        _ => None,
    };
    match operator {
        Comparison::Equal => values_equal(left, right),
        Comparison::NotEqual => !values_equal(left, right),
        Comparison::Less => ordering.is_some_and(|ordering| ordering.is_lt()),
        Comparison::LessEqual => ordering.is_some_and(|ordering| ordering.is_le()),
        Comparison::Greater => ordering.is_some_and(|ordering| ordering.is_gt()),
        Comparison::GreaterEqual => ordering.is_some_and(|ordering| ordering.is_ge()),
    }
}

fn contains(haystack: &Value, needle: &Value) -> bool {
    match (haystack, needle) {
        (Value::Array(items), needle) => items.iter().any(|item| values_equal(item, needle)),
        (Value::String(text), Value::String(part)) => text.contains(part.as_str()),
        _ => false,
    }
}

/// Reads a GeoJSON `properties` object into filter values; nested values stay as JSON.
pub fn properties_from_json(properties: Option<&Value>) -> HashMap<String, Value> {
    properties
        .and_then(Value::as_object)
        .map(|object| {
            object
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
