//! Type-annotation (`assert` / `coerce` insertion) and parse-time constant folding.

use super::{Parser, Result};
use crate::style::expression::{
    ast::{Coercion, Expression},
    evaluate::EvaluationContext,
    value::Type,
};

/// How a parsed expression is fitted to the type its context expects.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Annotation {
    /// Wrap an untyped value in an assertion, or a coercible value in its property coercion.
    Assert,
    /// Wrap an untyped value in a coercion, what GL JS does at the root of string properties.
    Coerce,
    /// Check the type but leave the expression bare; `coalesce` operands take this.
    Omit,
}

fn composite_coercion(expected: &Type, actual: &Type) -> Option<Coercion> {
    match expected {
        Type::Formatted
            if matches!(
                actual,
                Type::Value | Type::String | Type::ResolvedImage | Type::Null
            ) =>
        {
            Some(Coercion::Formatted)
        }
        Type::ResolvedImage if matches!(actual, Type::Value | Type::String) => {
            Some(Coercion::ResolvedImage)
        }
        Type::Color if matches!(actual, Type::Value | Type::String) => Some(Coercion::Color),
        Type::Padding => match actual {
            Type::Value | Type::Number => Some(Coercion::Padding),
            Type::Array { item, length }
                if matches!(**item, Type::Value | Type::Number)
                    && length.is_none_or(|len| len <= 4) =>
            {
                Some(Coercion::Padding)
            }
            _ => None,
        },
        Type::NumberArray => match actual {
            Type::Value | Type::Number => Some(Coercion::NumberArray),
            Type::Array { item, .. } if matches!(**item, Type::Value | Type::Number) => {
                Some(Coercion::NumberArray)
            }
            _ => None,
        },
        Type::ColorArray => match actual {
            Type::Value | Type::Color | Type::String => Some(Coercion::ColorArray),
            Type::Array { item, .. }
                if matches!(**item, Type::Value | Type::Color | Type::String) =>
            {
                Some(Coercion::ColorArray)
            }
            _ => None,
        },
        Type::ProjectionDefinition => match actual {
            Type::Value | Type::String | Type::Object => Some(Coercion::ProjectionDefinition),
            Type::Array { length, .. } if length.is_none_or(|len| len == 3) => {
                Some(Coercion::ProjectionDefinition)
            }
            _ => None,
        },
        Type::VariableAnchorOffsetCollection => match actual {
            Type::Value => Some(Coercion::VariableAnchorOffsetCollection),
            Type::Array { length, .. }
                if length.is_none_or(|len| len >= 2 && len.is_multiple_of(2)) =>
            {
                Some(Coercion::VariableAnchorOffsetCollection)
            }
            _ => None,
        },
        _ => None,
    }
}

impl Parser {
    pub(super) fn annotate(
        &self,
        parsed: Expression,
        expected: Option<&Type>,
        annotation: Annotation,
    ) -> Result<Expression> {
        let Some(expected) = expected else {
            return Ok(parsed);
        };
        let actual = parsed.output_type();
        let assertable = matches!(
            expected,
            Type::String | Type::Number | Type::Boolean | Type::Object | Type::Array { .. }
        );
        if assertable && actual == Type::Value {
            return Ok(match annotation {
                Annotation::Assert => Expression::Assert {
                    required: expected.clone(),
                    operands: vec![parsed],
                },
                Annotation::Coerce if *expected == Type::String => Expression::Coerce {
                    coercion: Coercion::String,
                    operands: vec![parsed],
                },
                Annotation::Coerce | Annotation::Omit => parsed,
            });
        }
        if let Some(coercion) = composite_coercion(expected, &actual) {
            return Ok(match annotation {
                Annotation::Omit => parsed,
                Annotation::Assert | Annotation::Coerce => Expression::Coerce {
                    coercion,
                    operands: vec![parsed],
                },
            });
        }
        if self.null_fallbacks && actual == Type::Null {
            return Ok(parsed);
        }
        if !actual.is_subtype_of(expected) {
            return Err(self.error(format!("Expected {expected} but found {actual} instead.")));
        }
        Ok(parsed)
    }

    /// Evaluates an expression that depends on nothing, as GL JS does while parsing, so an
    /// error in it surfaces once rather than per feature.
    pub(super) fn fold(&self, parsed: Expression) -> Result<Expression> {
        if matches!(
            parsed,
            Expression::Literal(_) | Expression::Folded { .. } | Expression::Error(_)
        ) || !parsed.is_constant()
        {
            return Ok(parsed);
        }
        let output = parsed.output_type();
        match parsed.evaluate(&EvaluationContext::default()) {
            Ok(value) => Ok(Expression::Folded { value, output }),
            Err(error) => Err(self.error(error.to_string())),
        }
    }
}
