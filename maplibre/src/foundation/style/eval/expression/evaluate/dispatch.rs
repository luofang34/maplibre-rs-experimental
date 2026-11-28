//! Dispatched evaluation helpers for [`super::Expression::evaluate`].

use super::{
    expect_bool, expect_color, expect_number, expect_object, expect_string, feature_geometry,
    interpolate_values,
    operations::{arithmetic, compare, index_of, math, searchable, slice},
    rgba, EvaluationContext, EvaluationError, Result,
};
use crate::style::expression::{
    ast::{interpolation::Interpolation, Expression, FeatureProperty, Global, StringFunction},
    geometry::feature_identity::{is_internal_property, GEOMETRY_TYPE_PROPERTY, ID_PROPERTY},
    value::{
        collation::{is_supported_script, Collation},
        ColorSpace, Value,
    },
};

impl Expression {
    pub(super) fn eval_leaf_or_feature(
        &self,
        context: &EvaluationContext<'_>,
    ) -> Option<Result<Value>> {
        let val = match self {
            Self::Literal(value) | Self::Folded { value, .. } => value.clone(),
            Self::Global(Global::Zoom) => Value::Number(context.zoom),
            Self::Global(Global::Elevation) => Value::Number(context.elevation),
            Self::Global(Global::HeatmapDensity) => Value::Number(context.heatmap_density),
            Self::Global(Global::LineProgress) => Value::Number(context.line_progress),
            Self::Accumulated => context.accumulated.cloned().unwrap_or(Value::Null),
            Self::GlobalState(key) => context
                .global_state
                .and_then(|state| state.get(key))
                .cloned()
                .unwrap_or(Value::Null),
            // Outside a filter, the feature's own properties carry its id and geometry type.
            Self::Feature(FeatureProperty::Id) => context
                .id
                .or_else(|| context.properties?.get(ID_PROPERTY))
                .cloned()
                .unwrap_or(Value::Null),
            Self::Feature(FeatureProperty::GeometryType) => context
                .geometry_type
                .map(Value::from)
                .or_else(|| context.properties?.get(GEOMETRY_TYPE_PROPERTY).cloned())
                .unwrap_or(Value::Null),
            Self::Feature(FeatureProperty::Properties) => Value::Object(
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
            Self::Within(area) => Value::Bool(
                feature_geometry(context).is_some_and(|geometry| geometry.is_within(area)),
            ),
            Self::Distance(other) => Value::Number(
                feature_geometry(context)
                    .and_then(|geometry| geometry.distance_to(other))
                    .unwrap_or(-1.0),
            ),
            _ => return None,
        };
        Some(Ok(val))
    }

    pub(super) fn eval_control_or_logic(
        &self,
        context: &EvaluationContext<'_>,
    ) -> Option<Result<Value>> {
        match self {
            Self::Get { key, object } => Some(Self::eval_get(key, object.as_deref(), context)),
            Self::Has { key, object } => Some(Self::eval_has(key, object.as_deref(), context)),
            Self::Var { bound, .. } => Some(bound.evaluate(context)),
            Self::Let { body, .. } => Some(body.evaluate(context)),
            Self::Case {
                branches, fallback, ..
            } => Some(Self::eval_case(branches, fallback, context)),
            Self::Match {
                input,
                input_type,
                cases,
                fallback,
                ..
            } => Some(Self::eval_match(
                input, input_type, cases, fallback, context,
            )),
            Self::Coalesce { operands, .. } => Some(Self::eval_coalesce(operands, context)),
            Self::Compare {
                operator,
                left,
                right,
                untyped,
                collator,
            } => Some(Self::eval_compare(
                *operator,
                left,
                right,
                *untyped,
                collator.as_deref(),
                context,
            )),
            Self::Collator {
                case_sensitive,
                diacritic_sensitive,
                locale,
            } => Some(Self::eval_collator(
                case_sensitive,
                diacritic_sensitive,
                locale.as_deref(),
                context,
            )),
            Self::All(operands) => Some(Self::eval_all(operands, context)),
            Self::Any(operands) => Some(Self::eval_any(operands, context)),
            Self::Not(operand) => Some(
                operand
                    .evaluate(context)
                    .and_then(|v| Ok(Value::Bool(!expect_bool(v)?))),
            ),
            _ => None,
        }
    }

    fn eval_get(
        key: &Self,
        object: Option<&Self>,
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
        key: &Self,
        object: Option<&Self>,
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
        branches: &[(Self, Self)],
        fallback: &Self,
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
        input: &Self,
        input_type: &crate::style::expression::value::Type,
        cases: &[(Vec<Value>, Self)],
        fallback: &Self,
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
        left: &Self,
        right: &Self,
        untyped: bool,
        collator: Option<&Self>,
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
        case_sensitive: &Self,
        diacritic_sensitive: &Self,
        locale: Option<&Self>,
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

    fn eval_all(operands: &[Self], context: &EvaluationContext<'_>) -> Result<Value> {
        for operand in operands {
            if !expect_bool(operand.evaluate(context)?)? {
                return Ok(Value::Bool(false));
            }
        }
        Ok(Value::Bool(true))
    }

    fn eval_any(operands: &[Self], context: &EvaluationContext<'_>) -> Result<Value> {
        for operand in operands {
            if expect_bool(operand.evaluate(context)?)? {
                return Ok(Value::Bool(true));
            }
        }
        Ok(Value::Bool(false))
    }

    pub(super) fn eval_other(&self, context: &EvaluationContext<'_>) -> Result<Value> {
        if let Some(result) = self.eval_collection_or_text(context) {
            return result;
        }
        match self {
            Self::NumberFormat { input, options } => {
                Self::eval_number_format(input, options, context)
            }
            Self::ResolvedLocale(collator) => Ok(match collator.evaluate(context)? {
                Value::Collator(collation) => Value::String(collation.resolved_locale()),
                _ => Value::Null,
            }),
            Self::IsSupportedScript(text) => Ok(Value::Bool(
                text.evaluate(context)?
                    .as_str()
                    .is_none_or(is_supported_script),
            )),
            Self::Arithmetic { operator, operands } => {
                let numbers = operands
                    .iter()
                    .map(|operand| expect_number(operand.evaluate(context)?))
                    .collect::<Result<Vec<f64>>>()?;
                Ok(Value::Number(arithmetic(*operator, &numbers)))
            }
            Self::Math { function, operand } => Ok(Value::Number(math(
                *function,
                expect_number(operand.evaluate(context)?)?,
            ))),
            Self::MinMax { max, operands } => Self::eval_min_max(*max, operands, context),
            Self::TypeOf(operand) => Ok(Value::String(operand.evaluate(context)?.type_of().name())),
            Self::Assert { required, operands } => Self::eval_assert(required, operands, context),
            Self::Image(name) => Self::eval_image(name, context),
            Self::Format(sections) => Self::eval_format(sections, context),
            Self::Coerce { coercion, operands } => Self::eval_coerce(*coercion, operands, context),
            Self::ToRgba(operand) => {
                let [r, g, b, a] = expect_color(operand.evaluate(context)?)?.straight();
                Ok(Value::Array(vec![
                    Value::Number(r * 255.0),
                    Value::Number(g * 255.0),
                    Value::Number(b * 255.0),
                    Value::Number(a),
                ]))
            }
            Self::Rgba(operands) => Self::eval_rgba(operands, context),
            Self::Interpolate {
                interpolation,
                space,
                input,
                stops,
                ..
            } => Self::eval_interpolate(*interpolation, *space, input, stops, context),
            Self::Step { input, stops, .. } => Self::eval_step(input, stops, context),
            Self::Error(message) => Err(EvaluationError::UserError {
                message: expect_string(message.evaluate(context)?)?,
            }),
            _ => Ok(Value::Null),
        }
    }

    fn eval_collection_or_text(&self, context: &EvaluationContext<'_>) -> Option<Result<Value>> {
        Some(match self {
            Self::In { needle, haystack } => Self::eval_in(needle, haystack, context),
            Self::IndexOf {
                needle,
                haystack,
                from,
            } => Self::eval_index_of(needle, haystack, from.as_deref(), context),
            Self::Slice {
                input, from, to, ..
            } => Self::eval_slice(input, from, to.as_deref(), context),
            Self::Length(operand) => match operand.evaluate(context) {
                Ok(Value::String(text)) => Ok(Value::Number(text.chars().count() as f64)),
                Ok(Value::Array(items)) => Ok(Value::Number(items.len() as f64)),
                Ok(other) => Err(EvaluationError::NotIndexable {
                    found: other.type_of(),
                }),
                Err(err) => Err(err),
            },
            Self::Concat(operands) => Self::eval_concat(operands, context),
            Self::StringCase { function, operand } => operand
                .evaluate(context)
                .and_then(expect_string)
                .map(|text| {
                    Value::String(match function {
                        StringFunction::Upcase => text.to_uppercase(),
                        StringFunction::Downcase => text.to_lowercase(),
                    })
                }),
            Self::At { index, array, .. } => Self::eval_at(index, array, context),
            Self::Split { input, delimiter } => Self::eval_split(input, delimiter, context),
            Self::Join { input, separator } => Self::eval_join(input, separator, context),
            _ => return None,
        })
    }

    fn eval_in(needle: &Self, haystack: &Self, context: &EvaluationContext<'_>) -> Result<Value> {
        let needle = searchable(needle.evaluate(context)?)?;
        let haystack = haystack.evaluate(context)?;
        if !haystack.truthy() {
            return Ok(Value::Bool(false));
        }
        Ok(Value::Bool(index_of(&needle, &haystack, 0)? >= 0))
    }

    fn eval_index_of(
        needle: &Self,
        haystack: &Self,
        from: Option<&Self>,
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
        input: &Self,
        from: &Self,
        to: Option<&Self>,
        context: &EvaluationContext<'_>,
    ) -> Result<Value> {
        let input = input.evaluate(context)?;
        let from = expect_number(from.evaluate(context)?)? as i64;
        let to = to
            .map(|to| expect_number(to.evaluate(context)?).map(|n| n as i64))
            .transpose()?;
        slice(input, from, to)
    }

    fn eval_concat(operands: &[Self], context: &EvaluationContext<'_>) -> Result<Value> {
        let mut text = String::new();
        for operand in operands {
            text.push_str(&operand.evaluate(context)?.to_display_string());
        }
        Ok(Value::String(text))
    }

    fn eval_min_max(
        max: bool,
        operands: &[Self],
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
        operands: &[Self],
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

    fn eval_rgba(operands: &[Self], context: &EvaluationContext<'_>) -> Result<Value> {
        let numbers = operands
            .iter()
            .map(|operand| expect_number(operand.evaluate(context)?))
            .collect::<Result<Vec<f64>>>()?;
        let [Some(&r), Some(&g), Some(&b)] = [numbers.first(), numbers.get(1), numbers.get(2)]
        else {
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
        let index = Self::stop_at_or_below(stops, input);
        let (Some(lo), Some(hi)) = (stops.get(index), stops.get(index + 1)) else {
            return first.1.evaluate(context);
        };
        let t = interpolation.factor(input, lo.0, hi.0);
        interpolate_values(lo.1.evaluate(context)?, hi.1.evaluate(context)?, t, space)
    }
}
