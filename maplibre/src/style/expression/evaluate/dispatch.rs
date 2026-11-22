//! Dispatched evaluation helpers for [`super::Expression::evaluate`].

use super::{
    coercions::{coerce, interpolate_values, rgba},
    expect_bool, expect_color, expect_number, expect_object, expect_string, feature_geometry,
    images,
    operations::{
        arithmetic, compare, eval_at, eval_join, eval_number_format, eval_split, index_of, math,
        searchable, slice, stop_at_or_below,
    },
    EvaluationContext, EvaluationError, Result,
};
use crate::style::expression::{
    ast::{Expression, FeatureProperty, Global, StringFunction},
    collation::{is_supported_script, Collation},
    feature_identity::{is_internal_property, GEOMETRY_TYPE_PROPERTY, ID_PROPERTY},
    interpolation::{ColorSpace, Interpolation},
    value::Value,
};

pub(super) fn eval_leaf_or_feature(
    expr: &Expression,
    context: &EvaluationContext<'_>,
) -> Option<Result<Value>> {
    let val = match expr {
        Expression::Literal(value) | Expression::Folded { value, .. } => value.clone(),
        Expression::Global(Global::Zoom) => Value::Number(context.zoom),
        Expression::Global(Global::Elevation) => Value::Number(context.elevation),
        Expression::Global(Global::HeatmapDensity) => Value::Number(context.heatmap_density),
        Expression::Global(Global::LineProgress) => Value::Number(context.line_progress),
        Expression::Accumulated => context.accumulated.cloned().unwrap_or(Value::Null),
        Expression::GlobalState(key) => context
            .global_state
            .and_then(|state| state.get(key))
            .cloned()
            .unwrap_or(Value::Null),
        // Outside a filter, the feature's own properties carry its id and geometry type.
        Expression::Feature(FeatureProperty::Id) => context
            .id
            .or_else(|| context.properties?.get(ID_PROPERTY))
            .cloned()
            .unwrap_or(Value::Null),
        Expression::Feature(FeatureProperty::GeometryType) => context
            .geometry_type
            .map(Value::from)
            .or_else(|| context.properties?.get(GEOMETRY_TYPE_PROPERTY).cloned())
            .unwrap_or(Value::Null),
        Expression::Feature(FeatureProperty::Properties) => Value::Object(
            context
                .properties
                .map(|properties| {
                    properties
                        .iter()
                        .filter(|(key, _)| !is_internal_property(key))
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect()
                })
                .unwrap_or_default(),
        ),
        Expression::Within(area) => {
            Value::Bool(feature_geometry(context).is_some_and(|geometry| geometry.is_within(area)))
        }
        Expression::Distance(other) => Value::Number(
            feature_geometry(context)
                .and_then(|geometry| geometry.distance_to(other))
                .unwrap_or(-1.0),
        ),
        _ => return None,
    };
    Some(Ok(val))
}

pub(super) fn eval_control_or_logic(
    expr: &Expression,
    context: &EvaluationContext<'_>,
) -> Option<Result<Value>> {
    match expr {
        Expression::Get { key, object } => Some(eval_get(key, object.as_deref(), context)),
        Expression::Has { key, object } => Some(eval_has(key, object.as_deref(), context)),
        Expression::Var { bound, .. } => Some(bound.evaluate(context)),
        Expression::Let { body, .. } => Some(body.evaluate(context)),
        Expression::Case {
            branches, fallback, ..
        } => Some(eval_case(branches, fallback, context)),
        Expression::Match {
            input,
            input_type,
            cases,
            fallback,
            ..
        } => Some(eval_match(input, input_type, cases, fallback, context)),
        Expression::Coalesce { operands, .. } => Some(images::coalesce(operands, context)),
        Expression::Compare {
            operator,
            left,
            right,
            untyped,
            collator,
        } => Some(eval_compare(
            *operator,
            left,
            right,
            *untyped,
            collator.as_deref(),
            context,
        )),
        Expression::Collator {
            case_sensitive,
            diacritic_sensitive,
            locale,
        } => Some(eval_collator(
            case_sensitive,
            diacritic_sensitive,
            locale.as_deref(),
            context,
        )),
        Expression::All(operands) => Some(eval_all(operands, context)),
        Expression::Any(operands) => Some(eval_any(operands, context)),
        Expression::Not(operand) => Some(
            operand
                .evaluate(context)
                .and_then(|v| Ok(Value::Bool(!expect_bool(v)?))),
        ),
        _ => None,
    }
}

fn eval_get(
    key: &Expression,
    object: Option<&Expression>,
    context: &EvaluationContext<'_>,
) -> Result<Value> {
    let key = expect_string(key.evaluate(context)?)?;
    Ok(match object {
        Some(object) => expect_object(object.evaluate(context)?)?
            .get(&key)
            .cloned()
            .unwrap_or(Value::Null),
        None => context
            .properties
            .and_then(|properties| properties.get(&key))
            .cloned()
            .unwrap_or(Value::Null),
    })
}

fn eval_has(
    key: &Expression,
    object: Option<&Expression>,
    context: &EvaluationContext<'_>,
) -> Result<Value> {
    let key = expect_string(key.evaluate(context)?)?;
    Ok(Value::Bool(match object {
        Some(object) => expect_object(object.evaluate(context)?)?.contains_key(&key),
        None => context
            .properties
            .is_some_and(|properties| properties.contains_key(&key)),
    }))
}

fn eval_case(
    branches: &[(Expression, Expression)],
    fallback: &Expression,
    context: &EvaluationContext<'_>,
) -> Result<Value> {
    for (condition, output) in branches {
        if expect_bool(condition.evaluate(context)?)? {
            return output.evaluate(context);
        }
    }
    fallback.evaluate(context)
}

fn eval_match(
    input: &Expression,
    input_type: &crate::style::expression::value::Type,
    cases: &[(Vec<Value>, Expression)],
    fallback: &Expression,
    context: &EvaluationContext<'_>,
) -> Result<Value> {
    let input = input.evaluate(context)?;
    if input.type_of() == *input_type {
        for (labels, output) in cases {
            if labels.contains(&input) {
                return output.evaluate(context);
            }
        }
    }
    fallback.evaluate(context)
}

fn eval_compare(
    operator: crate::style::expression::ast::Comparison,
    left: &Expression,
    right: &Expression,
    untyped: bool,
    collator: Option<&Expression>,
    context: &EvaluationContext<'_>,
) -> Result<Value> {
    let collation = match collator {
        Some(collator) => match collator.evaluate(context)? {
            Value::Collator(collation) => Some(collation),
            _ => None,
        },
        None => None,
    };
    compare(
        operator,
        left.evaluate(context)?,
        right.evaluate(context)?,
        untyped,
        collation.as_ref(),
    )
}

fn eval_collator(
    case_sensitive: &Expression,
    diacritic_sensitive: &Expression,
    locale: Option<&Expression>,
    context: &EvaluationContext<'_>,
) -> Result<Value> {
    Ok(Value::Collator(Collation {
        case_sensitive: case_sensitive.evaluate(context)?.truthy(),
        diacritic_sensitive: diacritic_sensitive.evaluate(context)?.truthy(),
        locale: match locale {
            Some(locale) => locale.evaluate(context)?.as_str().map(str::to_owned),
            None => None,
        },
    }))
}

fn eval_all(operands: &[Expression], context: &EvaluationContext<'_>) -> Result<Value> {
    for operand in operands {
        if !expect_bool(operand.evaluate(context)?)? {
            return Ok(Value::Bool(false));
        }
    }
    Ok(Value::Bool(true))
}

fn eval_any(operands: &[Expression], context: &EvaluationContext<'_>) -> Result<Value> {
    for operand in operands {
        if expect_bool(operand.evaluate(context)?)? {
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(false))
}

pub(super) fn eval_other(expr: &Expression, context: &EvaluationContext<'_>) -> Result<Value> {
    if let Some(result) = eval_collection_or_text(expr, context) {
        return result;
    }
    match expr {
        Expression::NumberFormat { input, options } => eval_number_format(input, options, context),
        Expression::ResolvedLocale(collator) => Ok(match collator.evaluate(context)? {
            Value::Collator(collation) => Value::String(collation.resolved_locale()),
            _ => Value::Null,
        }),
        Expression::IsSupportedScript(text) => Ok(Value::Bool(
            text.evaluate(context)?
                .as_str()
                .is_none_or(is_supported_script),
        )),
        Expression::Arithmetic { operator, operands } => {
            let numbers = operands
                .iter()
                .map(|operand| expect_number(operand.evaluate(context)?))
                .collect::<Result<Vec<f64>>>()?;
            Ok(Value::Number(arithmetic(*operator, &numbers)))
        }
        Expression::Math { function, operand } => Ok(Value::Number(math(
            *function,
            expect_number(operand.evaluate(context)?)?,
        ))),
        Expression::MinMax { max, operands } => eval_min_max(*max, operands, context),
        Expression::TypeOf(operand) => {
            Ok(Value::String(operand.evaluate(context)?.type_of().name()))
        }
        Expression::Assert { required, operands } => eval_assert(required, operands, context),
        Expression::Image(name) => images::image(name, context),
        Expression::Format(sections) => images::format(sections, context),
        Expression::Coerce { coercion, operands } => coerce(*coercion, operands, context),
        Expression::ToRgba(operand) => {
            let [r, g, b, a] = expect_color(operand.evaluate(context)?)?.straight();
            Ok(Value::Array(vec![
                Value::Number(r * 255.0),
                Value::Number(g * 255.0),
                Value::Number(b * 255.0),
                Value::Number(a),
            ]))
        }
        Expression::Rgba(operands) => eval_rgba(operands, context),
        Expression::Interpolate {
            interpolation,
            space,
            input,
            stops,
            ..
        } => eval_interpolate(*interpolation, *space, input, stops, context),
        Expression::Step { input, stops, .. } => eval_step(input, stops, context),
        Expression::Error(message) => Err(EvaluationError::UserError {
            message: expect_string(message.evaluate(context)?)?,
        }),
        _ => Ok(Value::Null),
    }
}

fn eval_collection_or_text(
    expr: &Expression,
    context: &EvaluationContext<'_>,
) -> Option<Result<Value>> {
    Some(match expr {
        Expression::In { needle, haystack } => eval_in(needle, haystack, context),
        Expression::IndexOf {
            needle,
            haystack,
            from,
        } => eval_index_of(needle, haystack, from.as_deref(), context),
        Expression::Slice {
            input, from, to, ..
        } => eval_slice(input, from, to.as_deref(), context),
        Expression::Length(operand) => match operand.evaluate(context) {
            Ok(Value::String(text)) => Ok(Value::Number(text.chars().count() as f64)),
            Ok(Value::Array(items)) => Ok(Value::Number(items.len() as f64)),
            Ok(other) => Err(EvaluationError::NotIndexable {
                found: other.type_of(),
            }),
            Err(err) => Err(err),
        },
        Expression::Concat(operands) => eval_concat(operands, context),
        Expression::StringCase { function, operand } => operand
            .evaluate(context)
            .and_then(expect_string)
            .map(|text| {
                Value::String(match function {
                    StringFunction::Upcase => text.to_uppercase(),
                    StringFunction::Downcase => text.to_lowercase(),
                })
            }),
        Expression::At { index, array, .. } => eval_at(index, array, context),
        Expression::Split { input, delimiter } => eval_split(input, delimiter, context),
        Expression::Join { input, separator } => eval_join(input, separator, context),
        _ => return None,
    })
}

fn eval_in(
    needle: &Expression,
    haystack: &Expression,
    context: &EvaluationContext<'_>,
) -> Result<Value> {
    let needle = searchable(needle.evaluate(context)?)?;
    let haystack = haystack.evaluate(context)?;
    if !haystack.truthy() {
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(index_of(&needle, &haystack, 0)? >= 0))
}

fn eval_index_of(
    needle: &Expression,
    haystack: &Expression,
    from: Option<&Expression>,
    context: &EvaluationContext<'_>,
) -> Result<Value> {
    let needle = searchable(needle.evaluate(context)?)?;
    let haystack = haystack.evaluate(context)?;
    let from = match from {
        Some(from) => expect_number(from.evaluate(context)?)? as i64,
        None => 0,
    };
    Ok(Value::Number(index_of(&needle, &haystack, from)? as f64))
}

fn eval_slice(
    input: &Expression,
    from: &Expression,
    to: Option<&Expression>,
    context: &EvaluationContext<'_>,
) -> Result<Value> {
    let input = input.evaluate(context)?;
    let from = expect_number(from.evaluate(context)?)? as i64;
    let to = to
        .map(|to| expect_number(to.evaluate(context)?).map(|n| n as i64))
        .transpose()?;
    slice(input, from, to)
}

fn eval_concat(operands: &[Expression], context: &EvaluationContext<'_>) -> Result<Value> {
    let mut text = String::new();
    for operand in operands {
        text.push_str(&operand.evaluate(context)?.to_display_string());
    }
    Ok(Value::String(text))
}

fn eval_min_max(
    max: bool,
    operands: &[Expression],
    context: &EvaluationContext<'_>,
) -> Result<Value> {
    let mut result = if max {
        f64::NEG_INFINITY
    } else {
        f64::INFINITY
    };
    for operand in operands {
        let number = expect_number(operand.evaluate(context)?)?;
        result = if max {
            result.max(number)
        } else {
            result.min(number)
        };
    }
    Ok(Value::Number(result))
}

fn eval_assert(
    required: &crate::style::expression::value::Type,
    operands: &[Expression],
    context: &EvaluationContext<'_>,
) -> Result<Value> {
    let last = operands.len().saturating_sub(1);
    for (index, operand) in operands.iter().enumerate() {
        let value = operand.evaluate(context)?;
        let found = value.type_of();
        if found.is_subtype_of(required) {
            return Ok(value);
        }
        if index == last {
            return Err(EvaluationError::Expected {
                expected: required.clone(),
                found,
            });
        }
    }
    Ok(Value::Null)
}

fn eval_rgba(operands: &[Expression], context: &EvaluationContext<'_>) -> Result<Value> {
    let numbers = operands
        .iter()
        .map(|operand| expect_number(operand.evaluate(context)?))
        .collect::<Result<Vec<f64>>>()?;
    let [Some(&r), Some(&g), Some(&b)] = [numbers.first(), numbers.get(1), numbers.get(2)] else {
        return Err(EvaluationError::InvalidRgba {
            value: format!("{numbers:?}"),
            reason: "expected 3 or 4 numeric arguments".to_owned(),
        });
    };
    rgba(r, g, b, numbers.get(3).copied()).map(Value::Color)
}

fn eval_interpolate(
    interpolation: Interpolation,
    space: ColorSpace,
    input: &Expression,
    stops: &[(f64, Expression)],
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
    let index = stop_at_or_below(stops, input);
    let (Some(lo), Some(hi)) = (stops.get(index), stops.get(index + 1)) else {
        return first.1.evaluate(context);
    };
    let t = interpolation.factor(input, lo.0, hi.0);
    interpolate_values(lo.1.evaluate(context)?, hi.1.evaluate(context)?, t, space)
}

fn eval_step(
    input: &Expression,
    stops: &[(f64, Expression)],
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
    let stop = stops.get(stop_at_or_below(stops, input)).unwrap_or(first);
    stop.1.evaluate(context)
}
