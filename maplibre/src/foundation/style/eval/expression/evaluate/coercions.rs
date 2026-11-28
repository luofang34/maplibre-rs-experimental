//! Runtime coercions, `rgba` construction, and value interpolation across scalar, array, and
//! composite style-spec types.

use super::{EvaluationContext, EvaluationError, Result};
use crate::style::expression::{
    ast::{Coercion, Expression},
    value::{interpolate_number, Color, ColorSpace, Type, Value},
};

fn format_invalid_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => crate::util::json::json_value_to_string(&other.to_json()),
    }
}

fn validate_rgba_channels(r: f64, g: f64, b: f64, a: Option<f64>) -> Option<&'static str> {
    if ![r, g, b]
        .iter()
        .all(|channel| (0.0..=255.0).contains(channel))
    {
        return Some("'r', 'g', and 'b' must be between 0 and 255");
    }
    if a.is_some_and(|alpha| !(0.0..=1.0).contains(&alpha)) {
        return Some("'a' must be between 0 and 1");
    }
    None
}

#[inline]
fn scale_rgb_channel(channel: f64) -> f64 {
    channel / 255.0
}

fn format_rgba_error_value(r: f64, g: f64, b: f64, a: Option<f64>) -> String {
    crate::util::json::json_value_to_string(
        &Value::Array(
            [Some(r), Some(g), Some(b), a]
                .into_iter()
                .flatten()
                .map(Value::Number)
                .collect(),
        )
        .to_json(),
    )
}

fn format_mismatch_reason(kind: &str, left: usize, right: usize) -> String {
    format!("mismatched {kind} lengths {left} and {right} in interpolate")
}

fn interpolate_number_items(
    from: &[Value],
    to: &[Value],
    t: f64,
) -> std::result::Result<Vec<Value>, Type> {
    from.iter()
        .zip(to)
        .map(|(from, to)| match (from, to) {
            (Value::Number(from), Value::Number(to)) => {
                Ok(Value::Number(interpolate_number(*from, *to, t)))
            }
            (other, _) => Err(other.type_of()),
        })
        .collect()
}

impl Expression {
    fn value_to_number(value: &Value) -> Option<f64> {
        match value {
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
        }
    }

    fn extract_rgba_numbers(items: &[Value]) -> Option<(f64, f64, f64, Option<f64>)> {
        let numbers: Vec<f64> = items.iter().filter_map(Value::as_number).collect();
        if numbers.len() != items.len() || !(3..=4).contains(&items.len()) {
            return None;
        }
        let [Some(&r), Some(&g), Some(&b)] = [numbers.first(), numbers.get(1), numbers.get(2)]
        else {
            return None;
        };
        Some((r, g, b, numbers.get(3).copied()))
    }

    fn eval_first_operand(operands: &[Self], context: &EvaluationContext) -> Result<Value> {
        operands
            .first()
            .ok_or(EvaluationError::Expected {
                expected: Type::Value,
                found: Type::Null,
            })?
            .evaluate(context)
    }

    fn coerce_composite<T>(
        kind: &'static str,
        operands: &[Self],
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

    pub(super) fn eval_coerce(
        coercion: Coercion,
        operands: &[Self],
        context: &EvaluationContext,
    ) -> Result<Value> {
        match coercion {
            Coercion::Boolean => Ok(Value::Bool(
                Self::eval_first_operand(operands, context)?.truthy(),
            )),
            Coercion::Formatted => Ok(super::images::to_formatted(Self::eval_first_operand(
                operands, context,
            )?)),
            Coercion::ResolvedImage => Ok(super::images::to_image(
                Self::eval_first_operand(operands, context)?,
                context,
            )),
            Coercion::String => Ok(Value::String(
                Self::eval_first_operand(operands, context)?.to_display_string(),
            )),
            Coercion::Padding => Self::coerce_composite(
                "padding",
                operands,
                context,
                Value::to_padding,
                Value::Padding,
            ),
            Coercion::NumberArray => Self::coerce_composite(
                "numberArray",
                operands,
                context,
                Value::to_number_array,
                Value::NumberArray,
            ),
            Coercion::ColorArray => Self::coerce_composite(
                "colorArray",
                operands,
                context,
                Value::to_color_array,
                Value::ColorArray,
            ),
            Coercion::ProjectionDefinition => Self::coerce_composite(
                "projectionDefinition",
                operands,
                context,
                Value::to_projection_definition,
                Value::ProjectionDefinition,
            ),
            Coercion::VariableAnchorOffsetCollection => Self::coerce_composite(
                "variableAnchorOffsetCollection",
                operands,
                context,
                Value::to_variable_anchor_offset_collection,
                Value::VariableAnchorOffsetCollection,
            ),
            Coercion::Number => Self::coerce_number(operands, context),
            Coercion::Color => Self::coerce_color(operands, context),
        }
    }

    fn coerce_number(operands: &[Self], context: &EvaluationContext) -> Result<Value> {
        let mut last = Value::Null;
        for operand in operands {
            last = operand.evaluate(context)?;
            if last.is_null() {
                return Ok(Value::Number(0.0));
            }
            if let Some(number) = Self::value_to_number(&last) {
                return Ok(Value::Number(number));
            }
        }
        Err(EvaluationError::NotANumber {
            value: crate::util::json::json_value_to_string(&last.to_json()),
        })
    }

    fn coerce_color(operands: &[Self], context: &EvaluationContext) -> Result<Value> {
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
                    if let Some((r, g, b, a)) = Self::extract_rgba_numbers(items) {
                        match rgba(r, g, b, a) {
                            Ok(color) => return Ok(Value::Color(color)),
                            Err(rgba_error) => error = Some(rgba_error),
                        }
                    } else {
                        error = Some(EvaluationError::InvalidRgba {
                            value: crate::util::json::json_value_to_string(&last.to_json()),
                            reason:
                                "expected an array containing either three or four numeric values"
                                    .to_string(),
                        });
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

/// A colour from `0..=255` channels and an optional alpha, validated as GL JS does.
pub(super) fn rgba(r: f64, g: f64, b: f64, a: Option<f64>) -> Result<Color> {
    if let Some(reason) = validate_rgba_channels(r, g, b, a) {
        return Err(EvaluationError::InvalidRgba {
            value: format_rgba_error_value(r, g, b, a),
            reason: reason.to_string(),
        });
    }
    Ok(Color::new(
        scale_rgb_channel(r),
        scale_rgb_channel(g),
        scale_rgb_channel(b),
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
                reason: format_mismatch_reason("numberArray", from.0.len(), to.0.len()),
            }),
        (Value::ColorArray(from), Value::ColorArray(to)) => from
            .interpolate(&to, t, space)
            .map(Value::ColorArray)
            .ok_or_else(|| EvaluationError::IncompatibleInterpolation {
                reason: format_mismatch_reason("colorArray", from.0.len(), to.0.len()),
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
        (Value::Array(from), Value::Array(to)) => interpolate_number_items(&from, &to, t)
            .map(Value::Array)
            .map_err(|found| EvaluationError::Expected {
                expected: Type::Number,
                found,
            }),
        (from, _) => Err(EvaluationError::Expected {
            expected: Type::Number,
            found: from.type_of(),
        }),
    }
}
