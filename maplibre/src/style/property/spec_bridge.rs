//! Bridges [`crate::style::spec`] descriptors and composite expression values into
//! [`PropertyValue`] and [`StyleProperty`].

use std::sync::Arc;

use super::{PropertyExpression, PropertyValue, StyleProperty, UnsupportedProperty};
use crate::style::{
    expression::{
        convert_token_string, ColorArray, Expression, LegacyPropertySpec, NumberArray, Padding,
        ParseError, ProjectionDefinition, PropertyKind, Value, VariableAnchorOffsetCollection,
    },
    spec::{lookup_layout_property, lookup_paint_property, SpecPropertyKind},
};

/// Resolves a [`LegacyPropertySpec`] from the generated style-spec v8 table for `(layer_type, kind, name)`.
pub fn lookup_legacy_property_spec(
    layer_type: &str,
    kind: SpecPropertyKind,
    name: &str,
) -> Option<LegacyPropertySpec> {
    let meta = match kind {
        SpecPropertyKind::Paint => lookup_paint_property(layer_type, name)?,
        SpecPropertyKind::Layout => lookup_layout_property(layer_type, name)?,
    };
    Some(meta.legacy_property_spec())
}

impl PropertyValue for Padding {
    fn spec() -> LegacyPropertySpec {
        LegacyPropertySpec::interpolated(PropertyKind::Padding)
    }

    fn from_value(value: &Value) -> Option<Self> {
        Self::from_value(value)
    }
}

impl PropertyValue for NumberArray {
    fn spec() -> LegacyPropertySpec {
        LegacyPropertySpec::interpolated(PropertyKind::NumberArray)
    }

    fn from_value(value: &Value) -> Option<Self> {
        Self::from_value(value)
    }
}

impl PropertyValue for ColorArray {
    fn spec() -> LegacyPropertySpec {
        LegacyPropertySpec::interpolated(PropertyKind::ColorArray)
    }

    fn from_value(value: &Value) -> Option<Self> {
        Self::from_value(value)
    }
}

impl PropertyValue for ProjectionDefinition {
    fn spec() -> LegacyPropertySpec {
        LegacyPropertySpec::interpolated(PropertyKind::ProjectionDefinition)
    }

    fn from_value(value: &Value) -> Option<Self> {
        Self::from_value(value)
    }
}

impl PropertyValue for VariableAnchorOffsetCollection {
    fn spec() -> LegacyPropertySpec {
        LegacyPropertySpec::interpolated(PropertyKind::VariableAnchorOffsetCollection)
    }

    fn from_value(value: &Value) -> Option<Self> {
        Self::from_value(value)
    }
}

impl<T: PropertyValue> StyleProperty<T> {
    /// Parses a property value against an explicit [`LegacyPropertySpec`].
    pub fn parse_with_spec(json: &serde_json::Value, spec: &LegacyPropertySpec) -> Self {
        if let Some(constant) = T::from_literal(json) {
            return Self::Constant(constant);
        }
        // A `{token}` string reads feature properties, as GL JS lowers it before parsing.
        let lowered = match json.as_str() {
            Some(text) if spec.tokens => convert_token_string(text),
            _ => json.clone(),
        };
        match Expression::parse_property(&lowered, spec) {
            Ok(Expression::Literal(value)) | Ok(Expression::Folded { value, .. }) => {
                match T::from_value(&value) {
                    Some(constant) => Self::Constant(constant),
                    None => Self::Unsupported(Arc::new(UnsupportedProperty {
                        source: json.clone(),
                        error: ParseError {
                            key: String::new(),
                            message: format!(
                                "expected {}, found {}",
                                spec.expected_type(),
                                value.type_of()
                            ),
                        },
                    })),
                }
            }
            Ok(expression) => Self::Expression(Arc::new(PropertyExpression {
                source: json.clone(),
                expression,
            })),
            Err(error) => {
                tracing::error!(source = %json, %error, "unsupported style property value");
                Self::Unsupported(Arc::new(UnsupportedProperty {
                    source: json.clone(),
                    error,
                }))
            }
        }
    }

    /// Parses a property value using the generated `style-spec-v8.json` descriptor for
    /// `(layer_type, kind, name)`, falling back to `T::spec()` when unlisted.
    pub fn parse_for_layer_property(
        layer_type: &str,
        kind: SpecPropertyKind,
        name: &str,
        json: &serde_json::Value,
    ) -> Self {
        let spec = lookup_legacy_property_spec(layer_type, kind, name).unwrap_or_else(T::spec);
        Self::parse_with_spec(json, &spec)
    }
}
