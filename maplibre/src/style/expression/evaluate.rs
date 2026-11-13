//! Evaluation of a parsed expression for one feature at one zoom.

use std::collections::HashMap;

use super::{
    ast::{Expression, FeatureProperty, Global},
    collation::{is_supported_script, Collation},
    feature_identity::{is_internal_property, GEOMETRY_TYPE_PROPERTY, ID_PROPERTY},
    geometry::{Geometry, GEOMETRY_PROPERTY},
    value::Value,
};

mod coercions;
mod error;
mod images;
mod operations;

use coercions::{coerce, interpolate_values, rgba};
pub use error::EvaluationError;
use operations::*;

/// The typed properties of a feature.
pub type FeatureProperties = HashMap<String, Value>;

/// What an expression can observe while it is evaluated.
#[derive(Clone, Copy, Debug, Default)]
pub struct EvaluationContext<'a> {
    /// Zoom the expression is evaluated at.
    pub zoom: f64,
    /// Terrain elevation in metres, for `elevation`.
    pub elevation: f64,
    /// Density of a heatmap at the colour being looked up, for `heatmap-density`.
    pub heatmap_density: f64,
    /// How far along its line the colour being looked up lies, for `line-progress`.
    pub line_progress: f64,
    /// Properties of the feature; none when the expression is not evaluated for a feature.
    pub properties: Option<&'a FeatureProperties>,
    /// Geometry kind of the feature, as `geometry-type` reports it.
    pub geometry_type: Option<&'a str>,
    /// Id of the feature, when the source assigned one.
    pub id: Option<&'a Value>,
    /// Accumulated cluster property value read by `accumulated`; `None` outside cluster reduction.
    pub accumulated: Option<&'a Value>,
    /// Map-level state read by `global-state`; the map keeps none, so it is normally absent.
    pub global_state: Option<&'a FeatureProperties>,
    /// The images a layout can draw, which `image` expressions are checked against; absent when
    /// every image is assumed to exist.
    pub available_images: Option<&'a dyn ImageSet>,
}

/// A set of image names.
pub trait ImageSet: std::fmt::Debug {
    /// Whether an image of that name exists.
    fn contains_image(&self, name: &str) -> bool;
}

impl<V: std::fmt::Debug> ImageSet for HashMap<String, V> {
    fn contains_image(&self, name: &str) -> bool {
        self.contains_key(name)
    }
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

/// The geometry a feature carries in its properties, when it carries one.
fn feature_geometry(context: &EvaluationContext<'_>) -> Option<Geometry> {
    let text = context.properties?.get(GEOMETRY_PROPERTY)?.as_str()?;
    Geometry::from_geojson(&serde_json::from_str(text).ok()?)
}

type Result<T> = std::result::Result<T, EvaluationError>;

impl Expression {
    /// Evaluates the expression.
    pub fn evaluate(&self, context: &EvaluationContext) -> Result<Value> {
        match self {
            Self::Literal(value) | Self::Folded { value, .. } => Ok(value.clone()),
            Self::Global(Global::Zoom) => Ok(Value::Number(context.zoom)),
            Self::Global(Global::Elevation) => Ok(Value::Number(context.elevation)),
            Self::Global(Global::HeatmapDensity) => Ok(Value::Number(context.heatmap_density)),
            Self::Global(Global::LineProgress) => Ok(Value::Number(context.line_progress)),
            Self::Accumulated => Ok(context.accumulated.cloned().unwrap_or(Value::Null)),
            Self::GlobalState(key) => Ok(context
                .global_state
                .and_then(|state| state.get(key))
                .cloned()
                .unwrap_or(Value::Null)),
            // Outside a filter, the feature's own properties carry its id and geometry type.
            Self::Feature(FeatureProperty::Id) => Ok(context
                .id
                .or_else(|| context.properties?.get(ID_PROPERTY))
                .cloned()
                .unwrap_or(Value::Null)),
            Self::Feature(FeatureProperty::GeometryType) => Ok(context
                .geometry_type
                .map(Value::from)
                .or_else(|| context.properties?.get(GEOMETRY_TYPE_PROPERTY).cloned())
                .unwrap_or(Value::Null)),
            Self::Feature(FeatureProperty::Properties) => Ok(Value::Object(
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
            Self::Coalesce { operands, .. } => images::coalesce(operands, context),
            Self::Compare {
                operator,
                left,
                right,
                untyped,
                collator,
            } => {
                let collation = match collator {
                    Some(collator) => match collator.evaluate(context)? {
                        Value::Collator(collation) => Some(collation),
                        _ => None,
                    },
                    None => None,
                };
                compare(
                    *operator,
                    left.evaluate(context)?,
                    right.evaluate(context)?,
                    *untyped,
                    collation.as_ref(),
                )
            }
            Self::Collator {
                case_sensitive,
                diacritic_sensitive,
                locale,
            } => Ok(Value::Collator(Collation {
                case_sensitive: case_sensitive.evaluate(context)?.truthy(),
                diacritic_sensitive: diacritic_sensitive.evaluate(context)?.truthy(),
                locale: match locale {
                    Some(locale) => locale.evaluate(context)?.as_str().map(str::to_owned),
                    None => None,
                },
            })),
            Self::Within(area) => Ok(Value::Bool(
                feature_geometry(context).is_some_and(|geometry| geometry.is_within(area)),
            )),
            Self::Distance(other) => Ok(Value::Number(
                feature_geometry(context)
                    .and_then(|geometry| geometry.distance_to(other))
                    .unwrap_or(-1.0),
            )),
            Self::NumberFormat { input, options } => eval_number_format(input, options, context),
            Self::ResolvedLocale(collator) => Ok(match collator.evaluate(context)? {
                Value::Collator(collation) => Value::String(collation.resolved_locale()),
                _ => Value::Null,
            }),
            Self::IsSupportedScript(text) => Ok(Value::Bool(
                text.evaluate(context)?
                    .as_str()
                    .is_none_or(is_supported_script),
            )),
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
            Self::Image(name) => images::image(name, context),
            Self::Format(sections) => images::format(sections, context),
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
            Self::Error(message) => Err(EvaluationError::UserError {
                message: expect_string(message.evaluate(context)?)?,
            }),
            Self::At { index, array, .. } => eval_at(index, array, context),
            Self::Split { input, delimiter } => eval_split(input, delimiter, context),
            Self::Join { input, separator } => eval_join(input, separator, context),
        }
    }
}
