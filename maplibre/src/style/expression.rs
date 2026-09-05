//! Style expressions: the typed language of data-driven and zoom-driven style properties.
//!
//! An expression is parsed once into an [`Expression`] tree, classified as feature or zoom
//! dependent, and evaluated per feature at bucket build or per frame. Legacy function objects
//! and legacy filters are lowered into the same tree, so one evaluator serves every property.
//! Semantics follow the GL JS style specification; the conformance suite vendored under the
//! crate's `tests` directory pins them.

mod ast;
mod evaluate;
mod interpolation;
mod legacy;
mod parse;
mod value;

pub use ast::{
    Arithmetic, Coercion, Comparison, Expression, FeatureProperty, Global, MathFunction,
    StringFunction,
};
pub use evaluate::{EvaluationContext, EvaluationError, FeatureProperties};
pub use interpolation::{interpolate_number, ColorSpace, Interpolation};
pub use legacy::{
    convert_filter, convert_function, is_expression_filter, LegacyPropertySpec, PropertyKind,
};
pub use parse::{is_expression, ParseError};
pub use value::{js_number, Color, Type, Value};

use parse::{Annotation, Parser};

impl Expression {
    /// Parses an expression with no expectation about its type.
    pub fn parse(json: &serde_json::Value) -> Result<Self, ParseError> {
        Parser::new().parse(json, None)
    }

    /// Parses an expression that must produce `expected`; a bare string for a colour property
    /// becomes the colour, and a property read is checked at run time.
    pub fn parse_for(json: &serde_json::Value, expected: &Type) -> Result<Self, ParseError> {
        Parser::new().parse(json, Some(expected))
    }

    /// Parses the value of a style property: an expression, a legacy function object, or a
    /// constant.
    pub fn parse_property(
        json: &serde_json::Value,
        spec: &LegacyPropertySpec,
    ) -> Result<Self, ParseError> {
        let expected = spec.expected_type();
        // GL JS coerces rather than asserts at the root of string properties, so a number
        // read from a feature becomes its text instead of an error.
        let annotation = if spec.kind == PropertyKind::String {
            Annotation::Coerce
        } else {
            Annotation::Assert
        };
        let expression = match json {
            serde_json::Value::Object(function) => Parser::for_legacy_function().parse_with(
                &convert_function(function, spec),
                Some(&expected),
                annotation,
            )?,
            // An array that does not start with an operator is a literal array value, such as
            // a light position or a font stack.
            serde_json::Value::Array(items) if !is_expression(items) => Parser::new().parse_with(
                &serde_json::json!(["literal", json]),
                Some(&expected),
                annotation,
            )?,
            other => Parser::new().parse_with(other, Some(&expected), annotation)?,
        };
        check_zoom_curve(&expression)?;
        Ok(expression)
    }

    /// Parses a layer filter, legacy or expression syntax, into a boolean expression.
    pub fn parse_filter(json: &serde_json::Value) -> Result<Self, ParseError> {
        Self::parse_for(&convert_filter(json), &Type::Boolean)
    }
}

/// The zoom curve a property expression may hold: at most one `step` or `interpolate` over
/// `["zoom"]`, at the root or under `let` and `coalesce`, as GL JS `findZoomCurve` allows.
fn check_zoom_curve(expression: &Expression) -> Result<(), ParseError> {
    match find_zoom_curve(expression)? {
        None if !expression.is_zoom_constant() => Err(ParseError {
            key: String::new(),
            message: "\"zoom\" expression may only be used as input to a top-level \"step\" or \"interpolate\" expression.".to_string(),
        }),
        _ => Ok(()),
    }
}

fn find_zoom_curve(expression: &Expression) -> Result<Option<&Expression>, ParseError> {
    let result = match expression {
        Expression::Let { body, .. } => find_zoom_curve(body)?,
        Expression::Coalesce { operands, .. } => {
            let mut found = None;
            for operand in operands {
                found = find_zoom_curve(operand)?;
                if found.is_some() {
                    break;
                }
            }
            found
        }
        Expression::Step { input, .. } | Expression::Interpolate { input, .. }
            if matches!(**input, Expression::Global(Global::Zoom)) =>
        {
            Some(expression)
        }
        _ => None,
    };
    let mut children = Vec::new();
    expression.for_each_child(&mut |child| children.push(child));
    for child in children {
        match (result, find_zoom_curve(child)?) {
            (None, Some(_)) => {
                return Err(ParseError {
                    key: String::new(),
                    message: "\"zoom\" expression may only be used as input to a top-level \"step\" or \"interpolate\" expression.".to_string(),
                })
            }
            (Some(found), Some(child_found)) if !std::ptr::eq(found, child_found) => {
                return Err(ParseError {
                    key: String::new(),
                    message: "Only one zoom-based \"step\" or \"interpolate\" subexpression may be used in an expression.".to_string(),
                })
            }
            (None, None) | (Some(_), _) => {}
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
