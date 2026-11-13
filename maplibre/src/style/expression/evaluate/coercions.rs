//! Runtime coercions, `rgba` construction, and value interpolation across scalar, array, and
//! composite style-spec types.

use super::{EvaluationContext, EvaluationError, Result};
use crate::style::expression::{
    ast::{Coercion, Expression},
    interpolation::{interpolate_number, ColorSpace},
    value::{
        Color, ColorArray, NumberArray, Padding, ProjectionDefinition, Type, Value,
        VariableAnchorOffsetCollection,
    },
};

fn format_invalid_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_json().to_string(),
    }
}

fn coerce_composite<T>(
    kind: &'static str,
    operands: &[Expression],
    context: &EvaluationContext,
    parse: fn(&Value) -> Option<T>,
    wrap: fn(T) -> Value,
) -> Result<Value> {
    let mut last = Value::Null;
    for operand in operands {
        last = operand.evaluate(context)?;
        if let Some(parsed) = parse(&last) {
            return Ok(wrap(parsed));
        }
    }
    Err(EvaluationError::InvalidComposite {
        kind,
        value: format_invalid_value(&last),
    })
}

pub(super) fn coerce(
    coercion: Coercion,
    operands: &[Expression],
    context: &EvaluationContext,
) -> Result<Value> {
    match coercion {
        Coercion::Boolean => Ok(Value::Bool(operands[0].evaluate(context)?.truthy())),
        Coercion::Formatted => Ok(super::images::to_formatted(operands[0].evaluate(context)?)),
        Coercion::ResolvedImage => Ok(super::images::to_image(
            operands[0].evaluate(context)?,
            context,
        )),
        Coercion::String => Ok(Value::String(
            operands[0].evaluate(context)?.to_display_string(),
        )),
        Coercion::Padding => coerce_composite(
            "padding",
            operands,
            context,
            Padding::from_value,
            Value::Padding,
        ),
        Coercion::NumberArray => coerce_composite(
            "numberArray",
            operands,
            context,
            NumberArray::from_value,
            Value::NumberArray,
        ),
        Coercion::ColorArray => coerce_composite(
            "colorArray",
            operands,
            context,
            ColorArray::from_value,
            Value::ColorArray,
        ),
        Coercion::ProjectionDefinition => coerce_composite(
            "projectionDefinition",
            operands,
            context,
            ProjectionDefinition::from_value,
            Value::ProjectionDefinition,
        ),
        Coercion::VariableAnchorOffsetCollection => coerce_composite(
            "variableAnchorOffsetCollection",
            operands,
            context,
            VariableAnchorOffsetCollection::from_value,
            Value::VariableAnchorOffsetCollection,
        ),
        Coercion::Number => {
            let mut last = Value::Null;
            for operand in operands {
                last = operand.evaluate(context)?;
                if last.is_null() {
                    return Ok(Value::Number(0.0));
                }
                let number = match &last {
                    Value::Bool(flag) => Some(f64::from(u8::from(*flag))),
                    Value::Number(number) => Some(*number),
                    Value::String(text) => {
                        let trimmed = text.trim();
                        if trimmed.is_empty() {
                            Some(0.0)
                        } else {
                            trimmed.parse::<f64>().ok()
                        }
                    }
                    _ => None,
                };
                if let Some(number) = number {
                    return Ok(Value::Number(number));
                }
            }
            Err(EvaluationError::NotANumber {
                value: last.to_json().to_string(),
            })
        }
        Coercion::Color => {
            let mut error = None;
            let mut last = Value::Null;
            for operand in operands {
                last = operand.evaluate(context)?;
                error = None;
                match &last {
                    Value::Color(color) => return Ok(Value::Color(*color)),
                    Value::String(text) => {
                        if let Some(color) = Color::parse(text) {
                            return Ok(Value::Color(color));
                        }
                    }
                    Value::Array(items) => {
                        let numbers: Vec<f64> = items.iter().filter_map(Value::as_number).collect();
                        if numbers.len() != items.len() || !(3..=4).contains(&items.len()) {
                            error = Some(EvaluationError::InvalidRgba {
                                value: last.to_json().to_string(),
                                reason: "expected an array containing either three or four numeric values"
                                    .to_string(),
                            });
                        } else {
                            match rgba(numbers[0], numbers[1], numbers[2], numbers.get(3).copied())
                            {
                                Ok(color) => return Ok(Value::Color(color)),
                                Err(rgba_error) => error = Some(rgba_error),
                            }
                        }
                    }
                    _ => {}
                }
            }
            Err(error.unwrap_or_else(|| EvaluationError::InvalidColor {
                value: format_invalid_value(&last),
            }))
        }
    }
}

/// A colour from `0..=255` channels and an optional alpha, validated as GL JS does.
pub(super) fn rgba(r: f64, g: f64, b: f64, a: Option<f64>) -> Result<Color> {
    let value = || {
        Value::Array(
            [Some(r), Some(g), Some(b), a]
                .into_iter()
                .flatten()
                .map(Value::Number)
                .collect(),
        )
        .to_json()
        .to_string()
    };
    if ![r, g, b]
        .iter()
        .all(|channel| (0.0..=255.0).contains(channel))
    {
        return Err(EvaluationError::InvalidRgba {
            value: value(),
            reason: "'r', 'g', and 'b' must be between 0 and 255".to_string(),
        });
    }
    if let Some(alpha) = a {
        if !(0.0..=1.0).contains(&alpha) {
            return Err(EvaluationError::InvalidRgba {
                value: value(),
                reason: "'a' must be between 0 and 1".to_string(),
            });
        }
    }
    Ok(Color::new(
        r / 255.0,
        g / 255.0,
        b / 255.0,
        a.unwrap_or(1.0),
    ))
}

pub(super) fn interpolate_values(
    from: Value,
    to: Value,
    t: f64,
    space: ColorSpace,
) -> Result<Value> {
    match (from, to) {
        (Value::Number(from), Value::Number(to)) => {
            Ok(Value::Number(interpolate_number(from, to, t)))
        }
        (Value::Color(from), Value::Color(to)) => {
            Ok(Value::Color(Color::interpolate(from, to, t, space)))
        }
        (Value::Padding(from), Value::Padding(to)) => Ok(Value::Padding(from.interpolate(to, t))),
        (Value::NumberArray(from), Value::NumberArray(to)) => from
            .interpolate(&to, t)
            .map(Value::NumberArray)
            .ok_or_else(|| EvaluationError::IncompatibleInterpolation {
                reason: format!(
                    "mismatched numberArray lengths {} and {} in interpolate",
                    from.0.len(),
                    to.0.len()
                ),
            }),
        (Value::ColorArray(from), Value::ColorArray(to)) => from
            .interpolate(&to, t, space)
            .map(Value::ColorArray)
            .ok_or_else(|| EvaluationError::IncompatibleInterpolation {
                reason: format!(
                    "mismatched colorArray lengths {} and {} in interpolate",
                    from.0.len(),
                    to.0.len()
                ),
            }),
        (Value::ProjectionDefinition(from), Value::ProjectionDefinition(to)) => {
            Ok(Value::ProjectionDefinition(from.interpolate(&to, t)))
        }
        (
            Value::VariableAnchorOffsetCollection(from),
            Value::VariableAnchorOffsetCollection(to),
        ) => from
            .interpolate(&to, t)
            .map(Value::VariableAnchorOffsetCollection)
            .ok_or_else(|| EvaluationError::IncompatibleInterpolation {
                reason: "incompatible variableAnchorOffsetCollection stops in interpolate"
                    .to_owned(),
            }),
        (Value::Array(from), Value::Array(to)) => Ok(Value::Array(
            from.iter()
                .zip(&to)
                .map(|(from, to)| match (from, to) {
                    (Value::Number(from), Value::Number(to)) => {
                        Ok(Value::Number(interpolate_number(*from, *to, t)))
                    }
                    (other, _) => Err(EvaluationError::Expected {
                        expected: Type::Number,
                        found: other.type_of(),
                    }),
                })
                .collect::<Result<Vec<Value>>>()?,
        )),
        (from, _) => Err(EvaluationError::Expected {
            expected: Type::Number,
            found: from.type_of(),
        }),
    }
}
