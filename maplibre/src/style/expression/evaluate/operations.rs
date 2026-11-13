//! The operations the evaluator applies: type expectations, comparisons, searches,
//! arithmetic, and stop lookups.

use super::{EvaluationContext, EvaluationError, Result};
use crate::style::expression::{
    ast::{Arithmetic, Comparison, Expression, MathFunction},
    collation::Collation,
    number_format,
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
    collation: Option<&Collation>,
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
        (Value::String(a), Value::String(b)) => Some(match collation {
            Some(collation) => collation.compare(a, b),
            None => a.cmp(b),
        }),
        _ => None,
    };
    let equal = match (&left, &right, collation) {
        (Value::String(a), Value::String(b), Some(collation)) => collation.compare(a, b).is_eq(),
        _ => left == right,
    };
    Ok(Value::Bool(match operator {
        Comparison::Equal => equal,
        Comparison::NotEqual => !equal,
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

pub(super) fn eval_number_format(
    input: &Expression,
    options: &[(String, Expression)],
    context: &EvaluationContext<'_>,
) -> Result<Value> {
    let number = expect_number(input.evaluate(context)?)?;
    let read = |name: &str| -> Result<Option<Value>> {
        options
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, option)| option.evaluate(context))
            .transpose()
    };
    let digits = |value: Option<Value>| {
        value
            .and_then(|value| value.as_number())
            .map(|digits| digits.clamp(0.0, 20.0) as usize)
    };
    let locale = read("locale")?.and_then(|value| value.as_str().map(str::to_owned));
    let currency = read("currency")?.and_then(|value| value.as_str().map(str::to_owned));
    let min = digits(read("min-fraction-digits")?);
    let max = digits(read("max-fraction-digits")?);
    Ok(Value::String(number_format::format(
        number,
        locale.as_deref(),
        currency.as_deref(),
        min,
        max,
    )))
}

pub(super) fn eval_at(
    index: &Expression,
    array: &Expression,
    context: &EvaluationContext<'_>,
) -> Result<Value> {
    let index = expect_number(index.evaluate(context)?)?;
    let items = match array.evaluate(context)? {
        Value::Array(items) => items,
        other => {
            return Err(EvaluationError::Expected {
                expected: Type::array(Type::Value, None),
                found: other.type_of(),
            });
        }
    };
    if !index.is_finite() || index.fract() != 0.0 {
        return Err(EvaluationError::InvalidArrayIndex {
            reason: format!("Array index must be an integer, but found {index} instead."),
        });
    }
    if index < 0.0 || (index as usize) >= items.len() {
        return Err(EvaluationError::InvalidArrayIndex {
            reason: format!(
                "Array index out of bounds: {index} is not within 0..{}.",
                items.len()
            ),
        });
    }
    Ok(items[index as usize].clone())
}

pub(super) fn eval_split(
    input: &Expression,
    delimiter: &Expression,
    context: &EvaluationContext<'_>,
) -> Result<Value> {
    let text = expect_string(input.evaluate(context)?)?;
    let delimiter = expect_string(delimiter.evaluate(context)?)?;
    let parts = if delimiter.is_empty() {
        text.chars()
            .map(|ch| Value::String(ch.to_string()))
            .collect()
    } else {
        text.split(&delimiter)
            .map(|part| Value::String(part.to_owned()))
            .collect()
    };
    Ok(Value::Array(parts))
}

pub(super) fn eval_join(
    input: &Expression,
    separator: &Expression,
    context: &EvaluationContext<'_>,
) -> Result<Value> {
    let items = match input.evaluate(context)? {
        Value::Array(items) => items,
        other => {
            return Err(EvaluationError::Expected {
                expected: Type::array(Type::Value, None),
                found: other.type_of(),
            });
        }
    };
    let separator = expect_string(separator.evaluate(context)?)?;
    let joined = items
        .iter()
        .map(Value::to_display_string)
        .collect::<Vec<_>>()
        .join(&separator);
    Ok(Value::String(joined))
}
