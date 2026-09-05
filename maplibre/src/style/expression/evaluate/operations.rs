//! The operations the evaluator applies: type expectations, comparisons, searches,
//! arithmetic, coercions and stop lookups.

use super::{EvaluationContext, EvaluationError, Result};
use crate::style::expression::{
    ast::{Arithmetic, Coercion, Comparison, Expression, MathFunction},
    interpolation::{interpolate_number, ColorSpace},
    value::{Color, Type, Value},
};

pub(super) fn expect_number(value: Value) -> Result<f64> {
    value.as_number().ok_or_else(|| EvaluationError::Expected {
        expected: Type::Number,
        found: value.type_of(),
    })
}

pub(super) fn expect_string(value: Value) -> Result<String> {
    match value {
        Value::String(text) => Ok(text),
        other => Err(EvaluationError::Expected {
            expected: Type::String,
            found: other.type_of(),
        }),
    }
}

pub(super) fn expect_bool(value: Value) -> Result<bool> {
    value.as_bool().ok_or_else(|| EvaluationError::Expected {
        expected: Type::Boolean,
        found: value.type_of(),
    })
}

pub(super) fn expect_color(value: Value) -> Result<Color> {
    match value {
        Value::Color(color) => Ok(color),
        other => Err(EvaluationError::Expected {
            expected: Type::Color,
            found: other.type_of(),
        }),
    }
}

pub(super) fn expect_object(value: Value) -> Result<std::collections::BTreeMap<String, Value>> {
    match value {
        Value::Object(members) => Ok(members),
        other => Err(EvaluationError::Expected {
            expected: Type::Object,
            found: other.type_of(),
        }),
    }
}

pub(super) fn searchable(value: Value) -> Result<Value> {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => Ok(value),
        other => Err(EvaluationError::NotSearchable {
            found: other.type_of(),
        }),
    }
}

/// Position of `needle` in a string or array from `from`, counted in characters for strings,
/// or minus one.
pub(super) fn index_of(needle: &Value, haystack: &Value, from: i64) -> Result<i64> {
    match haystack {
        Value::String(text) => {
            // JavaScript spells a null needle out and starts a string search no earlier than
            // its beginning.
            let needle = match needle {
                Value::Null => "null".to_string(),
                other => other.to_display_string(),
            };
            let characters: Vec<char> = text.chars().collect();
            let start = clamp_index(from.max(0), characters.len());
            let needle_chars: Vec<char> = needle.chars().collect();
            if needle_chars.is_empty() {
                return Ok(start as i64);
            }
            Ok(characters[start..]
                .windows(needle_chars.len())
                .position(|window| window == needle_chars.as_slice())
                .map_or(-1, |position| (position + start) as i64))
        }
        Value::Array(items) => {
            let start = clamp_index(from, items.len());
            Ok(items[start..]
                .iter()
                .position(|item| item == needle)
                .map_or(-1, |position| (position + start) as i64))
        }
        other => Err(EvaluationError::NotIndexable {
            found: other.type_of(),
        }),
    }
}

/// JavaScript index semantics: negative counts from the end, and the result stays in range.
pub(super) fn clamp_index(index: i64, length: usize) -> usize {
    if index < 0 {
        (length as i64 + index).max(0) as usize
    } else {
        (index as usize).min(length)
    }
}

pub(super) fn slice(input: Value, from: i64, to: Option<i64>) -> Result<Value> {
    match input {
        Value::String(text) => {
            let characters: Vec<char> = text.chars().collect();
            let start = clamp_index(from, characters.len());
            let end = to.map_or(characters.len(), |to| clamp_index(to, characters.len()));
            Ok(Value::String(
                characters[start..end.max(start)].iter().collect(),
            ))
        }
        Value::Array(items) => {
            let start = clamp_index(from, items.len());
            let end = to.map_or(items.len(), |to| clamp_index(to, items.len()));
            Ok(Value::Array(items[start..end.max(start)].to_vec()))
        }
        other => Err(EvaluationError::NotIndexable {
            found: other.type_of(),
        }),
    }
}

pub(super) fn compare(
    operator: Comparison,
    left: Value,
    right: Value,
    untyped: bool,
) -> Result<Value> {
    if operator.is_ordering() && untyped {
        let (left_type, right_type) = (left.type_of(), right.type_of());
        if left_type != right_type || !matches!(left_type, Type::String | Type::Number) {
            return Err(EvaluationError::Incomparable {
                operator: operator.symbol(),
                left: left_type,
                right: right_type,
            });
        }
    }
    let ordering = match (&left, &right) {
        (Value::Number(a), Value::Number(b)) => a.partial_cmp(b),
        (Value::String(a), Value::String(b)) => Some(a.cmp(b)),
        _ => None,
    };
    Ok(Value::Bool(match operator {
        Comparison::Equal => left == right,
        Comparison::NotEqual => left != right,
        Comparison::Less => ordering.is_some_and(|ordering| ordering.is_lt()),
        Comparison::LessEqual => ordering.is_some_and(|ordering| ordering.is_le()),
        Comparison::Greater => ordering.is_some_and(|ordering| ordering.is_gt()),
        Comparison::GreaterEqual => ordering.is_some_and(|ordering| ordering.is_ge()),
    }))
}

pub(super) fn arithmetic(operator: Arithmetic, numbers: &[f64]) -> f64 {
    match operator {
        Arithmetic::Add => numbers.iter().sum(),
        Arithmetic::Multiply => numbers.iter().product(),
        Arithmetic::Subtract => match numbers {
            [only] => -only,
            [first, second, ..] => first - second,
            [] => 0.0,
        },
        Arithmetic::Divide => numbers[0] / numbers[1],
        Arithmetic::Remainder => numbers[0] % numbers[1],
        Arithmetic::Power => numbers[0].powf(numbers[1]),
    }
}

pub(super) fn math(function: MathFunction, number: f64) -> f64 {
    match function {
        MathFunction::Sqrt => number.sqrt(),
        MathFunction::Ln => number.ln(),
        MathFunction::Log10 => number.log10(),
        MathFunction::Log2 => number.log2(),
        MathFunction::Sin => number.sin(),
        MathFunction::Cos => number.cos(),
        MathFunction::Tan => number.tan(),
        MathFunction::Asin => number.asin(),
        MathFunction::Acos => number.acos(),
        MathFunction::Atan => number.atan(),
        MathFunction::Abs => number.abs(),
        // JavaScript rounds halves towards positive infinity; mirroring negatives keeps
        // -2.5 at -3 as GL JS does.
        MathFunction::Round => {
            if number < 0.0 {
                -js_round(-number)
            } else {
                js_round(number)
            }
        }
        MathFunction::Floor => number.floor(),
        MathFunction::Ceil => number.ceil(),
    }
}

pub(super) fn js_round(number: f64) -> f64 {
    (number + 0.5).floor()
}

pub(super) fn coerce(
    coercion: Coercion,
    operands: &[Expression],
    context: &EvaluationContext,
) -> Result<Value> {
    match coercion {
        Coercion::Boolean => Ok(Value::Bool(operands[0].evaluate(context)?.truthy())),
        Coercion::String => Ok(Value::String(
            operands[0].evaluate(context)?.to_display_string(),
        )),
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
                value: match &last {
                    Value::String(text) => text.clone(),
                    other => other.to_json().to_string(),
                },
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

/// Index of the last stop whose input is at or below `input`; the caller keeps `input`
/// strictly inside the stop range.
pub(super) fn stop_at_or_below(stops: &[(f64, Expression)], input: f64) -> usize {
    let index = match stops
        .binary_search_by(|(stop, _)| stop.partial_cmp(&input).unwrap_or(std::cmp::Ordering::Less))
    {
        Ok(index) => index,
        Err(index) => index.saturating_sub(1),
    };
    index.min(stops.len().saturating_sub(2))
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
