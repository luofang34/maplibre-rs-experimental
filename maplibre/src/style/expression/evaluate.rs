//! Evaluation of a parsed expression for one feature at one zoom.

use std::collections::HashMap;

use thiserror::Error;

use super::{
    ast::{Expression, FeatureProperty, Global},
    value::{Type, Value},
};

/// The typed properties of a feature.
pub type FeatureProperties = HashMap<String, Value>;

/// What an expression can observe while it is evaluated.
#[derive(Clone, Copy, Debug, Default)]
pub struct EvaluationContext<'a> {
    /// Zoom the expression is evaluated at.
    pub zoom: f64,
    /// Terrain elevation in metres, for `elevation`.
    pub elevation: f64,
    /// Properties of the feature; none when the expression is not evaluated for a feature.
    pub properties: Option<&'a FeatureProperties>,
    /// Geometry kind of the feature, as `geometry-type` reports it.
    pub geometry_type: Option<&'a str>,
    /// Id of the feature, when the source assigned one.
    pub id: Option<&'a Value>,
    /// Map-level state read by `global-state`; the map keeps none, so it is normally absent.
    pub global_state: Option<&'a FeatureProperties>,
}

impl<'a> EvaluationContext<'a> {
    /// A context without a feature, for zoom-driven properties.
    pub fn at_zoom(zoom: f64) -> Self {
        Self {
            zoom,
            ..Self::default()
        }
    }

    /// A context for a feature.
    pub fn for_feature(zoom: f64, properties: &'a FeatureProperties) -> Self {
        Self {
            zoom,
            properties: Some(properties),
            ..Self::default()
        }
    }
}

/// Why an expression could not produce a value.
#[derive(Error, Debug, Clone, PartialEq)]
pub enum EvaluationError {
    /// A value had the wrong type for the operator that received it.
    #[error("expected value to be of type {expected}, but found {found} instead")]
    Expected {
        /// Type the operator needed.
        expected: Type,
        /// Type of the value it received.
        found: Type,
    },
    /// A value could not be read as a colour.
    #[error("could not parse color from value '{value}'")]
    InvalidColor {
        /// The value, as `to-string` would print it.
        value: String,
    },
    /// An `rgba` component was out of range.
    #[error("invalid rgba value {value}: {reason}")]
    InvalidRgba {
        /// The components.
        value: String,
        /// Which component and range failed.
        reason: String,
    },
    /// A value could not be read as a number.
    #[error("could not convert {value} to number")]
    NotANumber {
        /// The value, as JSON.
        value: String,
    },
    /// An ordering comparison received operands of different or unordered types.
    #[error("expected arguments for \"{operator}\" to be (string, string) or (number, number), but found ({left}, {right}) instead")]
    Incomparable {
        /// The operator.
        operator: &'static str,
        /// Type of the left operand.
        left: Type,
        /// Type of the right operand.
        right: Type,
    },
    /// `in` or `index-of` received a needle that cannot be searched for.
    #[error("expected first argument to be of type boolean, string, number or null, but found {found} instead")]
    NotSearchable {
        /// Type of the needle.
        found: Type,
    },
    /// A string or array operation received something else.
    #[error("expected value to be of type string or array, but found {found} instead")]
    NotIndexable {
        /// Type of the value.
        found: Type,
    },
    /// A `step` or `interpolate` input was not a number, such as the result of dividing by zero.
    #[error("input is not a number")]
    InputNotANumber,
}

mod operations;

use operations::*;

type Result<T> = std::result::Result<T, EvaluationError>;

impl Expression {
    /// Evaluates the expression.
    pub fn evaluate(&self, context: &EvaluationContext) -> Result<Value> {
        match self {
            Self::Literal(value) | Self::Folded { value, .. } => Ok(value.clone()),
            Self::Global(Global::Zoom) => Ok(Value::Number(context.zoom)),
            Self::Global(Global::Elevation) => Ok(Value::Number(context.elevation)),
            Self::GlobalState(key) => Ok(context
                .global_state
                .and_then(|state| state.get(key))
                .cloned()
                .unwrap_or(Value::Null)),
            Self::Feature(FeatureProperty::Id) => Ok(context.id.cloned().unwrap_or(Value::Null)),
            Self::Feature(FeatureProperty::GeometryType) => {
                Ok(context.geometry_type.map_or(Value::Null, Value::from))
            }
            Self::Feature(FeatureProperty::Properties) => Ok(Value::Object(
                context
                    .properties
                    .map(|properties| {
                        properties
                            .iter()
                            .map(|(key, value)| (key.clone(), value.clone()))
                            .collect()
                    })
                    .unwrap_or_default(),
            )),
            Self::Get { key, object } => {
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
            Self::Has { key, object } => {
                let key = expect_string(key.evaluate(context)?)?;
                Ok(Value::Bool(match object {
                    Some(object) => expect_object(object.evaluate(context)?)?.contains_key(&key),
                    None => context
                        .properties
                        .is_some_and(|properties| properties.contains_key(&key)),
                }))
            }
            Self::Var { bound, .. } => bound.evaluate(context),
            Self::Let { body, .. } => body.evaluate(context),
            Self::Case {
                branches, fallback, ..
            } => {
                for (condition, output) in branches {
                    if expect_bool(condition.evaluate(context)?)? {
                        return output.evaluate(context);
                    }
                }
                fallback.evaluate(context)
            }
            Self::Match {
                input,
                input_type,
                cases,
                fallback,
                ..
            } => {
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
            Self::Coalesce { operands, .. } => {
                let mut result = Value::Null;
                for operand in operands {
                    result = operand.evaluate(context)?;
                    if !result.is_null() {
                        break;
                    }
                }
                Ok(result)
            }
            Self::Compare {
                operator,
                left,
                right,
                untyped,
            } => compare(
                *operator,
                left.evaluate(context)?,
                right.evaluate(context)?,
                *untyped,
            ),
            Self::All(operands) => {
                for operand in operands {
                    if !expect_bool(operand.evaluate(context)?)? {
                        return Ok(Value::Bool(false));
                    }
                }
                Ok(Value::Bool(true))
            }
            Self::Any(operands) => {
                for operand in operands {
                    if expect_bool(operand.evaluate(context)?)? {
                        return Ok(Value::Bool(true));
                    }
                }
                Ok(Value::Bool(false))
            }
            Self::Not(operand) => Ok(Value::Bool(!expect_bool(operand.evaluate(context)?)?)),
            Self::In { needle, haystack } => {
                let needle = searchable(needle.evaluate(context)?)?;
                let haystack = haystack.evaluate(context)?;
                if !haystack.truthy() {
                    return Ok(Value::Bool(false));
                }
                Ok(Value::Bool(index_of(&needle, &haystack, 0)? >= 0))
            }
            Self::IndexOf {
                needle,
                haystack,
                from,
            } => {
                let needle = searchable(needle.evaluate(context)?)?;
                let haystack = haystack.evaluate(context)?;
                let from = match from {
                    Some(from) => expect_number(from.evaluate(context)?)? as i64,
                    None => 0,
                };
                Ok(Value::Number(index_of(&needle, &haystack, from)? as f64))
            }
            Self::Slice {
                input, from, to, ..
            } => {
                let input = input.evaluate(context)?;
                let from = expect_number(from.evaluate(context)?)? as i64;
                let to = match to {
                    Some(to) => Some(expect_number(to.evaluate(context)?)? as i64),
                    None => None,
                };
                slice(input, from, to)
            }
            Self::Length(operand) => match operand.evaluate(context)? {
                Value::String(text) => Ok(Value::Number(text.chars().count() as f64)),
                Value::Array(items) => Ok(Value::Number(items.len() as f64)),
                other => Err(EvaluationError::NotIndexable {
                    found: other.type_of(),
                }),
            },
            Self::Arithmetic { operator, operands } => {
                let numbers = operands
                    .iter()
                    .map(|operand| expect_number(operand.evaluate(context)?))
                    .collect::<Result<Vec<f64>>>()?;
                Ok(Value::Number(arithmetic(*operator, &numbers)))
            }
            Self::Math { function, operand } => {
                let number = expect_number(operand.evaluate(context)?)?;
                Ok(Value::Number(math(*function, number)))
            }
            Self::MinMax { max, operands } => {
                let mut result = if *max {
                    f64::NEG_INFINITY
                } else {
                    f64::INFINITY
                };
                for operand in operands {
                    let number = expect_number(operand.evaluate(context)?)?;
                    result = if *max {
                        result.max(number)
                    } else {
                        result.min(number)
                    };
                }
                Ok(Value::Number(result))
            }
            Self::TypeOf(operand) => Ok(Value::String(operand.evaluate(context)?.type_of().name())),
            Self::Assert { required, operands } => {
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
            Self::Coerce { coercion, operands } => coerce(*coercion, operands, context),
            Self::ToRgba(operand) => {
                let color = expect_color(operand.evaluate(context)?)?;
                let [r, g, b, a] = color.straight();
                Ok(Value::Array(vec![
                    Value::Number(r * 255.0),
                    Value::Number(g * 255.0),
                    Value::Number(b * 255.0),
                    Value::Number(a),
                ]))
            }
            Self::Rgba(operands) => {
                let numbers = operands
                    .iter()
                    .map(|operand| expect_number(operand.evaluate(context)?))
                    .collect::<Result<Vec<f64>>>()?;
                let alpha = numbers.get(3).copied();
                rgba(numbers[0], numbers[1], numbers[2], alpha).map(Value::Color)
            }
            Self::Interpolate {
                interpolation,
                space,
                input,
                stops,
                ..
            } => {
                if stops.len() == 1 {
                    return stops[0].1.evaluate(context);
                }
                let input = expect_number(input.evaluate(context)?)?;
                if input.is_nan() {
                    return Err(EvaluationError::InputNotANumber);
                }
                if input <= stops[0].0 {
                    return stops[0].1.evaluate(context);
                }
                let last = stops.len() - 1;
                if input >= stops[last].0 {
                    return stops[last].1.evaluate(context);
                }
                let index = stop_at_or_below(stops, input);
                let (lower, upper) = (stops[index].0, stops[index + 1].0);
                let t = interpolation.factor(input, lower, upper);
                let from = stops[index].1.evaluate(context)?;
                let to = stops[index + 1].1.evaluate(context)?;
                interpolate_values(from, to, t, *space)
            }
            Self::Step { input, stops, .. } => {
                if stops.len() == 1 {
                    return stops[0].1.evaluate(context);
                }
                let input = expect_number(input.evaluate(context)?)?;
                if input.is_nan() {
                    return Err(EvaluationError::InputNotANumber);
                }
                if input <= stops[0].0 {
                    return stops[0].1.evaluate(context);
                }
                let last = stops.len() - 1;
                if input >= stops[last].0 {
                    return stops[last].1.evaluate(context);
                }
                stops[stop_at_or_below(stops, input)].1.evaluate(context)
            }
            Self::Concat(operands) => {
                let mut text = String::new();
                for operand in operands {
                    text.push_str(&operand.evaluate(context)?.to_display_string());
                }
                Ok(Value::String(text))
            }
            Self::StringCase { function, operand } => {
                let text = expect_string(operand.evaluate(context)?)?;
                Ok(Value::String(match function {
                    super::ast::StringFunction::Upcase => text.to_uppercase(),
                    super::ast::StringFunction::Downcase => text.to_lowercase(),
                }))
            }
        }
    }
}
