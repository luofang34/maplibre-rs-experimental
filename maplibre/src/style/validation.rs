//! Diagnostics for unsupported layer types, properties and expression evaluation contexts.

use thiserror::Error;

use crate::style::{
    expression::{Expression, FeatureProperty, Global, ParseError},
    filter::{Filter, FilterError},
    layer::StyleLayer,
    property::{PropertyValue, StyleProperty},
    Style,
};

mod paint;
pub(crate) mod symbol;

/// One thing in a style the renderer would otherwise skip silently.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum StyleValidationError {
    /// A layer's filter uses syntax outside the supported subset; the layer renders nothing.
    #[error("layer `{layer}` has an unsupported filter: {source}")]
    Filter {
        /// The id of the layer carrying the filter.
        layer: String,
        /// Why the filter cannot be evaluated.
        #[source]
        source: FilterError,
    },
    /// The renderer has no path for this layer type.
    #[error("layer `{layer}` has unsupported type `{kind}`")]
    LayerType {
        /// The layer ID.
        layer: String,
        /// The requested layer type.
        kind: String,
    },
    /// A property cannot be applied as written.
    #[error("layer `{layer}` property `{property}`: {source}")]
    Property {
        /// The layer ID.
        layer: String,
        /// The property path within the layer.
        property: String,
        /// The unsupported syntax or rendering behavior.
        #[source]
        source: PropertyValidationError,
    },
}

/// Why the renderer cannot honor a property.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PropertyValidationError {
    /// The expression parser rejected this value.
    #[error("{source}")]
    Expression {
        /// The expression's error, including its operand path.
        #[source]
        source: ParseError,
    },
    /// A supported syntax requests rendering behavior that is not implemented.
    #[error("{reason}")]
    Unsupported {
        /// The specific limitation.
        reason: &'static str,
    },
}

impl Style {
    /// Checks layer types, retained paint/layout properties, filters and property expressions
    /// against their rendering paths. This is not a full style-spec or source-data validator;
    /// expressions can still fail for individual features at evaluation time.
    pub fn validate(&self) -> Result<(), Vec<StyleValidationError>> {
        let mut errors = Vec::new();
        for layer in &self.layers {
            LayerValidation {
                layer,
                errors: &mut errors,
            }
            .validate();
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Logs each validation error; the map still loads and renders what it can.
    pub fn log_validation_errors(&self) {
        if let Err(errors) = self.validate() {
            for error in errors {
                tracing::error!(%error, "style uses a construct this renderer does not support");
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Evaluation {
    Constant,
    Zoom,
    Feature,
    Elevation,
    Filter,
}

struct LayerValidation<'a> {
    layer: &'a StyleLayer,
    errors: &'a mut Vec<StyleValidationError>,
}

impl LayerValidation<'_> {
    fn validate(&mut self) {
        if !matches!(
            self.layer.type_.as_str(),
            "background"
                | "fill"
                | "line"
                | "circle"
                | "symbol"
                | "raster"
                | "hillshade"
                | "color-relief"
        ) {
            self.errors.push(StyleValidationError::LayerType {
                layer: self.layer.id.clone(),
                kind: self.layer.type_.clone(),
            });
        }
        for (scope, properties) in [
            ("paint", &self.layer.unrecognized.paint),
            ("layout", &self.layer.unrecognized.layout),
        ] {
            for name in properties.keys() {
                self.unsupported(&format!("{scope}.{name}"), "property is not implemented");
            }
        }
        if let Some(value) = &self.layer.filter {
            match Filter::parse(value) {
                Err(source) => self.errors.push(StyleValidationError::Filter {
                    layer: self.layer.id.clone(),
                    source,
                }),
                Ok(filter) => {
                    if let Some(reason) = missing_input(filter.expression(), Evaluation::Filter) {
                        self.unsupported("filter", reason);
                    }
                }
            }
        }
        if let Some(paint) = &self.layer.paint {
            self.paint(paint);
        }
    }

    fn error(&mut self, property: &str, source: PropertyValidationError) {
        self.errors.push(StyleValidationError::Property {
            layer: self.layer.id.clone(),
            property: property.into(),
            source,
        });
    }

    fn unsupported(&mut self, property: &str, reason: &'static str) {
        self.error(property, PropertyValidationError::Unsupported { reason });
    }

    fn property<T: PropertyValue>(
        &mut self,
        name: &str,
        property: Option<&StyleProperty<T>>,
        evaluation: Evaluation,
    ) {
        let Some(property) = property else {
            return;
        };
        if let StyleProperty::Unsupported(value) = property {
            self.error(
                name,
                PropertyValidationError::Expression {
                    source: value.error.clone(),
                },
            );
        } else if let Some(reason) = property
            .expression()
            .and_then(|expression| missing_input(expression, evaluation))
        {
            self.unsupported(name, reason);
        } else if !matches!(evaluation, Evaluation::Feature) && !property.is_feature_constant() {
            self.unsupported(
                name,
                "this rendering path does not evaluate per-feature values",
            );
        } else if matches!(evaluation, Evaluation::Constant | Evaluation::Elevation)
            && !property.is_zoom_constant()
        {
            self.unsupported(name, "this rendering path requires a constant value");
        }
    }
}

fn missing_input(expression: &Expression, evaluation: Evaluation) -> Option<&'static str> {
    let mut missing = match expression {
        Expression::Feature(FeatureProperty::Id | FeatureProperty::GeometryType)
            if !matches!(evaluation, Evaluation::Filter) =>
        {
            Some("this property evaluation has no feature ID or geometry type")
        }
        Expression::Global(Global::Elevation) if !matches!(evaluation, Evaluation::Elevation) => {
            Some("this evaluation has no terrain elevation")
        }
        _ => None,
    };
    expression.for_each_child(&mut |child| {
        if missing.is_none() {
            missing = missing_input(child, evaluation);
        }
    });
    missing
}

#[cfg(test)]
mod tests;
