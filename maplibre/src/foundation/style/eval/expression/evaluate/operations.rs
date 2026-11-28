//! The operations the evaluator applies: type expectations, comparisons, searches,
//! arithmetic, and stop lookups.

use super::{number_format, EvaluationContext, EvaluationError, Result};
use crate::style::expression::{
    ast::{Arithmetic, Comparison, Expression, MathFunction},
    value::{collation::Collation, Color, Type, Value},
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

pub(super) fn expect_object(value: Value) -> Result<rustc_hash::FxHashMap<String, Value>> {
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

fn index_of_string(needle: &Value, text: &str, from: i64) -> i64 {
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
        return start as i64;
    }
    characters
        .get(start..)
        .unwrap_or(&[])
        .windows(needle_chars.len())
        .position(|window| window == needle_chars.as_slice())
        .map_or(-1, |position| (position + start) as i64)
}

fn index_of_array(needle: &Value, items: &[Value], from: i64) -> i64 {
    let start = clamp_index(from, items.len());
    items
        .get(start..)
        .unwrap_or(&[])
        .iter()
        .position(|item| item == needle)
        .map_or(-1, |position| (position + start) as i64)
}

/// Position of `needle` in a string or array from `from`, counted in characters for strings,
/// or minus one.
pub(super) fn index_of(needle: &Value, haystack: &Value, from: i64) -> Result<i64> {
    match haystack {
        Value::String(text) => Ok(index_of_string(needle, text, from)),
        Value::Array(items) => Ok(index_of_array(needle, items, from)),
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

fn slice_bounds(from: i64, to: Option<i64>, len: usize) -> std::ops::Range<usize> {
    let start = clamp_index(from, len);
    let end = to.map_or(len, |to| clamp_index(to, len)).max(start);
    start..end
}

pub(super) fn slice(input: Value, from: i64, to: Option<i64>) -> Result<Value> {
    match input {
        Value::String(text) => {
            let characters: Vec<char> = text.chars().collect();
            let range = slice_bounds(from, to, characters.len());
            Ok(Value::String(
                characters.get(range).unwrap_or(&[]).iter().collect(),
            ))
        }
        Value::Array(items) => {
            let range = slice_bounds(from, to, items.len());
            Ok(Value::Array(items.get(range).unwrap_or(&[]).to_vec()))
        }
        other => Err(EvaluationError::NotIndexable {
            found: other.type_of(),
        }),
    }
}

fn incomparable_types(operator: Comparison, left: &Value, right: &Value) -> Option<(Type, Type)> {
    if !operator.is_ordering() {
        return None;
    }
    let (left_type, right_type) = (left.type_of(), right.type_of());
    if left_type != right_type || !matches!(left_type, Type::String | Type::Number) {
        Some((left_type, right_type))
    } else {
        None
    }
}

fn compare_values_ordering(
    left: &Value,
    right: &Value,
    collation: Option<&Collation>,
) -> Option<std::cmp::Ordering> {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => a.partial_cmp(b),
        (Value::String(a), Value::String(b)) => Some(match collation {
            Some(collation) => collation.compare(a, b),
            None => a.cmp(b),
        }),
        _ => None,
    }
}

fn eval_comparison_flag(
    operator: Comparison,
    equal: bool,
    ordering: Option<std::cmp::Ordering>,
) -> bool {
    match operator {
        Comparison::Equal => equal,
        Comparison::NotEqual => !equal,
        Comparison::Less => ordering.is_some_and(|ordering| ordering.is_lt()),
        Comparison::LessEqual => ordering.is_some_and(|ordering| ordering.is_le()),
        Comparison::Greater => ordering.is_some_and(|ordering| ordering.is_gt()),
        Comparison::GreaterEqual => ordering.is_some_and(|ordering| ordering.is_ge()),
    }
}

fn values_equal(left: &Value, right: &Value, collation: Option<&Collation>) -> bool {
    match (left, right, collation) {
        (Value::String(a), Value::String(b), Some(collation)) => collation.compare(a, b).is_eq(),
        _ => left == right,
    }
}

pub(super) fn compare(
    operator: Comparison,
    left: Value,
    right: Value,
    untyped: bool,
    collation: Option<&Collation>,
) -> Result<Value> {
    if untyped {
        if let Some((left, right)) = incomparable_types(operator, &left, &right) {
            return Err(EvaluationError::Incomparable {
                operator: operator.symbol(),
                left,
                right,
            });
        }
    }
    let ordering = compare_values_ordering(&left, &right, collation);
    let equal = values_equal(&left, &right, collation);
    Ok(Value::Bool(eval_comparison_flag(operator, equal, ordering)))
}

fn apply_binary_float_op(operator: Arithmetic, numbers: &[f64]) -> f64 {
    match (operator, numbers) {
        (Arithmetic::Divide, [a, b, ..]) => a / b,
        (Arithmetic::Remainder, [a, b, ..]) => a % b,
        (Arithmetic::Power, [a, b, ..]) => a.powf(*b),
        (Arithmetic::Power, _) => 1.0,
        _ => 0.0,
    }
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
        Arithmetic::Divide | Arithmetic::Remainder | Arithmetic::Power => {
            apply_binary_float_op(operator, numbers)
        }
    }
}

fn round_js(number: f64) -> f64 {
    if number < 0.0 {
        -js_round(-number)
    } else {
        js_round(number)
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
        MathFunction::Round => round_js(number),
        MathFunction::Floor => number.floor(),
        MathFunction::Ceil => number.ceil(),
    }
}

pub(super) fn js_round(number: f64) -> f64 {
    (number + 0.5).floor()
}

impl Expression {
    fn into_array_or_type(value: Value) -> std::result::Result<Vec<Value>, Type> {
        match value {
            Value::Array(items) => Ok(items),
            other => Err(other.type_of()),
        }
    }

    fn parse_fraction_digits(value: Option<Value>) -> Option<usize> {
        value
            .and_then(|value| value.as_number())
            .map(|digits| digits.clamp(0.0, 20.0) as usize)
    }

    fn extract_string_option(value: Option<Value>) -> Option<String> {
        value.and_then(|value| value.as_str().map(str::to_owned))
    }

    fn invalid_array_index_reason(index: f64, len: usize) -> Option<String> {
        if !index.is_finite() || index.fract() != 0.0 {
            return Some(format!(
                "Array index must be an integer, but found {index} instead."
            ));
        }
        if index < 0.0 || (index as usize) >= len {
            return Some(format!(
                "Array index out of bounds: {index} is not within 0..{len}."
            ));
        }
        None
    }

    fn split_string_values(text: &str, delimiter: &str) -> Vec<Value> {
        if delimiter.is_empty() {
            text.chars()
                .map(|ch| Value::String(ch.to_string()))
                .collect()
        } else {
            text.split(delimiter)
                .map(|part| Value::String(part.to_owned()))
                .collect()
        }
    }

    fn eval_format_option(
        options: &[(String, Self)],
        name: &str,
        context: &EvaluationContext<'_>,
    ) -> Result<Option<Value>> {
        options
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, option)| option.evaluate(context))
            .transpose()
    }

    /// Index of the last stop whose input is at or below `input`; the caller keeps `input`
    /// strictly inside the stop range.
    pub(super) fn stop_at_or_below(stops: &[(f64, Self)], input: f64) -> usize {
        let index = match stops.binary_search_by(|(stop, _)| {
            stop.partial_cmp(&input).unwrap_or(std::cmp::Ordering::Less)
        }) {
            Ok(index) => index,
            Err(index) => index.saturating_sub(1),
        };
        index.min(stops.len().saturating_sub(2))
    }

    pub(super) fn eval_number_format(
        input: &Self,
        options: &[(String, Self)],
        context: &EvaluationContext<'_>,
    ) -> Result<Value> {
        let number = expect_number(input.evaluate(context)?)?;
        let locale =
            Self::extract_string_option(Self::eval_format_option(options, "locale", context)?);
        let currency =
            Self::extract_string_option(Self::eval_format_option(options, "currency", context)?);
        let min = Self::parse_fraction_digits(Self::eval_format_option(
            options,
            "min-fraction-digits",
            context,
        )?);
        let max = Self::parse_fraction_digits(Self::eval_format_option(
            options,
            "max-fraction-digits",
            context,
        )?);
        Ok(Value::String(number_format::format(
            number,
            locale.as_deref(),
            currency.as_deref(),
            min,
            max,
        )))
    }

    pub(super) fn eval_at(
        index: &Self,
        array: &Self,
        context: &EvaluationContext<'_>,
    ) -> Result<Value> {
        let index = expect_number(index.evaluate(context)?)?;
        let items = Self::into_array_or_type(array.evaluate(context)?).map_err(|found| {
            EvaluationError::Expected {
                expected: Type::array(Type::Value, None),
                found,
            }
        })?;
        if let Some(reason) = Self::invalid_array_index_reason(index, items.len()) {
            return Err(EvaluationError::InvalidArrayIndex { reason });
        }
        Ok(items.get(index as usize).cloned().unwrap_or(Value::Null))
    }

    pub(super) fn eval_split(
        input: &Self,
        delimiter: &Self,
        context: &EvaluationContext<'_>,
    ) -> Result<Value> {
        let text = expect_string(input.evaluate(context)?)?;
        let delimiter = expect_string(delimiter.evaluate(context)?)?;
        Ok(Value::Array(Self::split_string_values(&text, &delimiter)))
    }

    pub(super) fn eval_join(
        input: &Self,
        separator: &Self,
        context: &EvaluationContext<'_>,
    ) -> Result<Value> {
        let items = Self::into_array_or_type(input.evaluate(context)?).map_err(|found| {
            EvaluationError::Expected {
                expected: Type::array(Type::Value, None),
                found,
            }
        })?;
        let separator = expect_string(separator.evaluate(context)?)?;
        let joined = items
            .iter()
            .map(Value::to_display_string)
            .collect::<Vec<_>>()
            .join(&separator);
        Ok(Value::String(joined))
    }

    pub(super) fn eval_step(
        input: &Self,
        stops: &[(f64, Self)],
        context: &EvaluationContext<'_>,
    ) -> Result<Value> {
        let Some(first) = stops.first() else {
            return Ok(Value::Null);
        };
        if stops.len() == 1 {
            return first.1.evaluate(context);
        }
        let input = expect_number(input.evaluate(context)?)?;
        if input.is_nan() {
            return Err(EvaluationError::InputNotANumber);
        }
        if input <= first.0 {
            return first.1.evaluate(context);
        }
        if let Some(last) = stops.last().filter(|last| input >= last.0) {
            return last.1.evaluate(context);
        }
        let stop = stops
            .get(Self::stop_at_or_below(stops, input))
            .unwrap_or(first);
        stop.1.evaluate(context)
    }
}
