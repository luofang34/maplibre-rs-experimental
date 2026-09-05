//! Feature filters of style layers: the legacy filter syntax and expression filters, lowered
//! into one boolean [`Expression`] and evaluated per feature.
//!
//! Legacy filters name a property directly (`["==", "level", "low"]`) while expression filters
//! read it through an operand (`["==", ["get", "level"], "low"]`). Both forms compare typed
//! values, so a number never equals its string spelling and a missing property is unequal to
//! everything. A filter the engine cannot parse is an error rather than a filter that quietly
//! admits or drops features.

use serde_json::Value as Json;
use thiserror::Error;

use crate::style::expression::{
    EvaluationContext, Expression, FeatureProperties, ParseError, Value,
};

/// Why a filter cannot be evaluated.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum FilterError {
    /// The filter is not a valid boolean expression.
    #[error("invalid filter: {source}")]
    Invalid {
        /// What the expression engine rejected.
        #[source]
        source: ParseError,
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
    /// Points and multi-points.
    Point,
    /// Lines and multi-lines.
    LineString,
    /// Polygons and multi-polygons.
    Polygon,
    /// A geometry the source did not classify.
    Unknown,
}

impl GeometryType {
    /// The geometry kind of an MVT feature type code.
    pub fn from_mvt(kind: i32) -> Self {
        match kind {
            1 => Self::Point,
            2 => Self::LineString,
            3 => Self::Polygon,
            _ => Self::Unknown,
        }
    }

    /// The geometry kind of a GeoJSON geometry type name.
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
            Self::Point => "Point",
            Self::LineString => "LineString",
            Self::Polygon => "Polygon",
            Self::Unknown => "Unknown",
        }
    }
}

/// What a filter can observe about one feature.
pub struct FeatureContext<'a> {
    /// The feature's typed properties.
    pub properties: &'a FeatureProperties,
    /// The feature's geometry kind.
    pub geometry_type: GeometryType,
    /// The feature's id, when the source assigned one.
    pub id: Option<Value>,
    /// Zoom of the tile being processed, which is what GL JS hands to filters.
    pub zoom: f64,
}

/// A layer filter, parsed once.
#[derive(Debug, Clone, PartialEq)]
pub struct Filter {
    expression: Expression,
}

impl Filter {
    /// Parses a layer's `filter` value; `null` and an empty array pass every feature.
    pub fn parse(value: &Json) -> Result<Self, FilterError> {
        let expression = match value {
            Json::Null => Expression::Literal(Value::Bool(true)),
            Json::Array(items) if items.is_empty() => Expression::Literal(Value::Bool(true)),
            Json::Bool(_) | Json::Array(_) => {
                Expression::parse_filter(value).map_err(|source| FilterError::Invalid { source })?
            }
            other => {
                return Err(FilterError::NotAFilter {
                    found: kind_name(other).to_string(),
                })
            }
        };
        Ok(Self { expression })
    }

    /// Whether the feature passes the filter; a filter that fails to evaluate drops the
    /// feature, as GL JS does.
    pub fn evaluate(&self, feature: &FeatureContext) -> bool {
        let context = EvaluationContext {
            zoom: feature.zoom,
            elevation: 0.0,
            properties: Some(feature.properties),
            geometry_type: Some(feature.geometry_type.name()),
            id: feature.id.as_ref(),
            global_state: None,
        };
        self.expression
            .evaluate(&context)
            .ok()
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
    }

    /// The boolean expression the filter evaluates.
    pub fn expression(&self) -> &Expression {
        &self.expression
    }
}

fn kind_name(value: &Json) -> &'static str {
    match value {
        Json::Null => "null",
        Json::Bool(_) => "boolean",
        Json::Number(_) => "number",
        Json::String(_) => "string",
        Json::Array(_) => "array",
        Json::Object(_) => "object",
    }
}

/// Typed properties from a GeoJSON `properties` object.
pub fn properties_from_json(properties: Option<&Json>) -> FeatureProperties {
    properties
        .and_then(Json::as_object)
        .map(|object| {
            object
                .iter()
                .map(|(key, value)| (key.clone(), Value::from_json(value)))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
