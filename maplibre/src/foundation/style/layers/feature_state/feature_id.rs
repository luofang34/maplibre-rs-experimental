//! Queried feature identifiers and effective expression ID resolution.

use serde::Serialize;

use crate::style::expression::{js_number, FeatureProperties, Value};

/// A feature identifier returned by rendered-feature or source queries: a non-negative integer
/// or a string (such as a string `promoteId` or GeoJSON string `id`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum QueriedFeatureId {
    /// Whole-number feature identifier.
    Number(u64),
    /// String feature identifier.
    String(String),
}

impl QueriedFeatureId {
    /// Parses a JSON feature id or promoted property value.
    pub fn from_json(value: &serde_json::Value) -> Option<Self> {
        if let Some(n) = value.as_u64() {
            return Some(Self::Number(n));
        }
        if let Some(n) = value.as_f64() {
            if let Some(id) = queried_id_from_f64(n) {
                return Some(id);
            }
        }
        value.as_str().map(|s| Self::String(s.to_owned()))
    }

    /// Parses an evaluated expression value into a feature identifier.
    pub(crate) fn from_expr_value(value: &Value) -> Option<Self> {
        match value {
            Value::Number(n) => queried_id_from_f64(*n),
            Value::String(s) => Some(Self::String(s.clone())),
            _ => None,
        }
    }

    /// Converts this identifier into an expression [`Value`] for `["id"]` / `$id` filters.
    pub(crate) fn to_expr_value(&self) -> Value {
        match self {
            Self::Number(n) => Value::Number(*n as f64),
            Self::String(s) => Value::String(s.clone()),
        }
    }
}

impl From<u64> for QueriedFeatureId {
    fn from(value: u64) -> Self {
        Self::Number(value)
    }
}

impl From<String> for QueriedFeatureId {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for QueriedFeatureId {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

impl PartialEq<u64> for QueriedFeatureId {
    fn eq(&self, other: &u64) -> bool {
        matches!(self, Self::Number(n) if n == other)
    }
}

impl PartialEq<&str> for QueriedFeatureId {
    fn eq(&self, other: &&str) -> bool {
        matches!(self, Self::String(s) if s == other)
    }
}

#[inline]
fn queried_id_from_f64(n: f64) -> Option<QueriedFeatureId> {
    if n >= 0.0 && n.fract() == 0.0 && n < 9.0e15 {
        Some(QueriedFeatureId::Number(n as u64))
    } else if n.is_finite() {
        Some(QueriedFeatureId::String(js_number(n)))
    } else {
        None
    }
}

/// Resolves the effective expression `["id"]` / `$id` value for a feature, respecting `promoteId`.
/// When `promote_property` is `Some`, it overrides `raw_id` without falling back to it.
pub(crate) fn resolve_effective_expr_id(
    raw_id: Option<u64>,
    properties: &FeatureProperties,
    promote_property: Option<&str>,
) -> Option<Value> {
    if let Some(prop) = promote_property {
        return match properties.get(prop) {
            Some(Value::String(s)) => Some(Value::String(s.clone())),
            Some(Value::Number(n)) if n.is_finite() => Some(Value::Number(*n)),
            _ => None,
        };
    }
    raw_id.map(|id| Value::Number(id as f64))
}
