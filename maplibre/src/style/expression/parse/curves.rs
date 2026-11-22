//! Parsing of the branching and ramp operators: `let`, `case`, `match`, `coalesce`, `step`
//! and `interpolate`.

use serde_json::Value as Json;

use super::{json_kind, Parser, Result};
use crate::style::expression::{
    ast::Expression,
    interpolation::{ColorSpace, Interpolation},
    value::{Type, Value},
};

impl Parser {
    pub(super) fn parse_let(
        &mut self,
        args: &[Json],
        expected: Option<&Type>,
    ) -> Result<Expression> {
        if args.len() < 3 {
            return Err(self.error(format!(
                "Expected at least 3 arguments, but found {} instead.",
                args.len()
            )));
        }
        let Some((body_arg, init_args)) = args.split_last() else {
            return Err(self.error("Expected at least 3 arguments."));
        };
        let mut bindings = Vec::new();
        for (offset, pair) in init_args.chunks(2).enumerate() {
            let index = offset * 2 + 1;
            let Some(name_json) = pair.first() else {
                continue;
            };
            let Some(name) = name_json.as_str() else {
                return Err(self.error(format!(
                    "Expected string, but found {} instead.",
                    json_kind(name_json)
                )));
            };
            if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(
                    self.error("Variable names must contain only alphanumeric characters or '_'.")
                );
            }
            let Some(value) = pair.get(1) else {
                return Err(self.error("Expected an odd number of arguments."));
            };
            let bound =
                self.with_bindings(&bindings, |parser| parser.parse_at(value, index + 1, None))?;
            bindings.push((name.to_string(), bound));
        }
        let body = self.with_bindings(&bindings, |parser| {
            parser.parse_at(body_arg, args.len(), expected)
        })?;
        Ok(Expression::Let {
            bindings,
            body: Box::new(body),
        })
    }

    pub(super) fn parse_case(
        &mut self,
        args: &[Json],
        expected: Option<&Type>,
    ) -> Result<Expression> {
        if args.len() < 3 {
            return Err(self.error(format!(
                "Expected at least 3 arguments, but found only {}.",
                args.len()
            )));
        }
        if args.len().is_multiple_of(2) {
            return Err(self.error("Expected an odd number of arguments."));
        }
        let Some((fallback_arg, branch_args)) = args.split_last() else {
            return Err(self.error("Expected at least 3 arguments."));
        };
        let mut output: Option<Type> = expected.cloned();
        let mut branches = Vec::new();
        for (offset, pair) in branch_args.chunks(2).enumerate() {
            let [cond_json, res_json] = pair else {
                continue;
            };
            let index = offset * 2 + 1;
            let condition = self.parse_at(cond_json, index, Some(&Type::Boolean))?;
            let result = self.parse_at(res_json, index + 1, output.as_ref())?;
            if output.is_none() && result.output_type() != Type::Error {
                output = Some(result.output_type());
            }
            branches.push((condition, result));
        }
        let fallback = self.parse_at(fallback_arg, args.len(), output.as_ref())?;
        let output = output.unwrap_or_else(|| fallback.output_type());
        Ok(Expression::Case {
            branches,
            fallback: Box::new(fallback),
            output,
        })
    }

    pub(super) fn parse_match(
        &mut self,
        args: &[Json],
        expected: Option<&Type>,
    ) -> Result<Expression> {
        if args.len() < 4 {
            return Err(self.error(format!(
                "Expected at least 4 arguments, but found only {}.",
                args.len()
            )));
        }
        if !args.len().is_multiple_of(2) {
            return Err(self.error("Expected an even number of arguments."));
        }
        let mut input_type: Option<Type> = None;
        let mut output: Option<Type> = expected.cloned();
        let mut cases: Vec<(Vec<Value>, Expression)> = Vec::new();
        let mut seen: Vec<Value> = Vec::new();
        let last = args.len() - 1;
        let middle = args.get(1..last).unwrap_or(&[]);
        for (offset, pair) in middle.chunks(2).enumerate() {
            let (Some(label_json), Some(result_json)) = (pair.first(), pair.get(1)) else {
                continue;
            };
            let values = self.parse_match_labels(label_json, &mut input_type, &mut seen)?;
            let result = self.parse_at(result_json, offset * 2 + 3, output.as_ref())?;
            if output.is_none() && result.output_type() != Type::Error {
                output = Some(result.output_type());
            }
            cases.push((values, result));
        }
        let input_arg = args
            .first()
            .ok_or_else(|| self.error("Missing match input argument."))?;
        let fallback_arg = args
            .get(last)
            .ok_or_else(|| self.error("Missing match fallback argument."))?;
        let input = self.parse_at(input_arg, 1, Some(&Type::Value))?;
        let fallback = self.parse_at(fallback_arg, last + 1, output.as_ref())?;
        let input_type = input_type.unwrap_or(Type::Value);
        let actual = input.output_type();
        if actual != Type::Value && !actual.is_subtype_of(&input_type) {
            return Err(self.error(format!("Expected {input_type} but found {actual} instead.")));
        }
        let output = output.unwrap_or_else(|| fallback.output_type());
        Ok(Expression::Match {
            input: Box::new(input),
            input_type,
            cases,
            fallback: Box::new(fallback),
            output,
        })
    }

    fn parse_match_labels(
        &self,
        label_json: &Json,
        input_type: &mut Option<Type>,
        seen: &mut Vec<Value>,
    ) -> Result<Vec<Value>> {
        let labels: Vec<&Json> = match label_json {
            Json::Array(labels) => labels.iter().collect(),
            label => vec![label],
        };
        if labels.is_empty() {
            return Err(self.error("Expected at least one branch label."));
        }
        let mut values = Vec::with_capacity(labels.len());
        for label in labels {
            let value = match label {
                Json::String(text) => Value::String(text.clone()),
                Json::Number(number) => {
                    let number = number.as_f64().unwrap_or(f64::NAN);
                    if number.abs() > 9007199254740991.0 {
                        return Err(self.error(
                            "Branch labels must be integers no larger than 9007199254740991.",
                        ));
                    }
                    if number.floor() != number {
                        return Err(self.error("Numeric branch labels must be integer values."));
                    }
                    Value::Number(number)
                }
                _ => return Err(self.error("Branch labels must be numbers or strings.")),
            };
            match input_type {
                None => *input_type = Some(value.type_of()),
                Some(expected) if *expected != value.type_of() => {
                    return Err(self.error(format!(
                        "Expected {expected} but found {} instead.",
                        value.type_of()
                    )))
                }
                Some(_) => {}
            }
            if seen.contains(&value) {
                return Err(self.error("Branch labels must be unique."));
            }
            seen.push(value.clone());
            values.push(value);
        }
        Ok(values)
    }

    pub(super) fn parse_coalesce(
        &mut self,
        args: &[Json],
        expected: Option<&Type>,
    ) -> Result<Expression> {
        if args.is_empty() {
            return Err(self.error("Expected at least one argument."));
        }
        let mut output: Option<Type> = expected.cloned();
        let mut operands = Vec::new();
        for (offset, arg) in args.iter().enumerate() {
            let operand = self.parse_unwrapped_at(arg, offset + 1, output.as_ref())?;
            if output.is_none() && operand.output_type() != Type::Error {
                output = Some(operand.output_type());
            }
            operands.push(operand);
        }
        let output = output.unwrap_or(Type::Value);
        // An operand the expected type cannot vouch for leaves the whole `coalesce` untyped,
        // so the context wraps the result once instead of every operand.
        let needs_annotation = expected.is_some_and(|expected| {
            operands
                .iter()
                .any(|operand| !operand.output_type().is_subtype_of(expected))
        });
        Ok(Expression::Coalesce {
            operands,
            output: if needs_annotation {
                Type::Value
            } else {
                output
            },
        })
    }

    pub(super) fn parse_step(
        &mut self,
        args: &[Json],
        expected: Option<&Type>,
    ) -> Result<Expression> {
        if args.len() < 4 {
            return Err(self.error(format!(
                "Expected at least 4 arguments, but found only {}.",
                args.len()
            )));
        }
        if !args.len().is_multiple_of(2) {
            return Err(self.error("Expected an even number of arguments."));
        }
        let Some((input_arg, stop_args)) = args.split_first() else {
            return Err(self.error("Expected at least 4 arguments."));
        };
        let input = self.parse_at(input_arg, 1, Some(&Type::Number))?;
        let (stops, output) = self.parse_stops(stop_args, 2, "step", expected.cloned())?;
        Ok(Expression::Step {
            input: Box::new(input),
            stops,
            output,
        })
    }

    pub(super) fn parse_interpolate(
        &mut self,
        operator: &str,
        args: &[Json],
        expected: Option<&Type>,
    ) -> Result<Expression> {
        let Some(Json::Array(curve)) = args.first() else {
            return Err(self.error("Expected an interpolation type expression."));
        };
        let interpolation = self.parse_interpolation_type(curve)?;
        if args.len() < 4 {
            return Err(self.error(format!(
                "Expected at least 4 arguments, but found only {}.",
                args.len()
            )));
        }
        if !args.len().is_multiple_of(2) {
            return Err(self.error("Expected an even number of arguments."));
        }
        let input_arg = args
            .get(1)
            .ok_or_else(|| self.error("Missing interpolation input argument."))?;
        let input = self.parse_at(input_arg, 2, Some(&Type::Number))?;
        let space = match operator {
            "interpolate-lab" => ColorSpace::Lab,
            "interpolate-hcl" => ColorSpace::Hcl,
            _ => ColorSpace::Rgb,
        };
        let seed = if space != ColorSpace::Rgb {
            Some(if expected == Some(&Type::ColorArray) {
                Type::ColorArray
            } else {
                Type::Color
            })
        } else {
            expected.cloned()
        };
        let stop_args = args.get(2..).unwrap_or(&[]);
        let (stops, output) = self.parse_stops(stop_args, 3, "interpolate", seed)?;
        if !is_interpolatable_type(space, &output) {
            return Err(self.error(format!("Type {output} is not interpolatable.")));
        }
        Ok(Expression::Interpolate {
            interpolation,
            space,
            input: Box::new(input),
            stops,
            output,
        })
    }

    fn parse_interpolation_type(&self, curve: &[Json]) -> Result<Interpolation> {
        match curve.first().and_then(Json::as_str) {
            Some("linear") => Ok(Interpolation::Linear),
            Some("exponential") => match curve.get(1).and_then(Json::as_f64) {
                Some(base) => Ok(Interpolation::Exponential { base }),
                None => Err(self.error("Exponential interpolation requires a numeric base.")),
            },
            Some("cubic-bezier") => {
                let points: Vec<f64> = curve
                    .get(1..)
                    .unwrap_or(&[])
                    .iter()
                    .filter_map(Json::as_f64)
                    .collect();
                let [Some(&p0), Some(&p1), Some(&p2), Some(&p3)] =
                    [points.first(), points.get(1), points.get(2), points.get(3)]
                else {
                    return Err(self.error(
                        "Cubic bezier interpolation requires four numeric arguments with values between 0 and 1.",
                    ));
                };
                if points.len() != 4
                    || curve.len() != 5
                    || points.iter().any(|point| !(0.0..=1.0).contains(point))
                {
                    return Err(self.error(
                        "Cubic bezier interpolation requires four numeric arguments with values between 0 and 1.",
                    ));
                }
                Ok(Interpolation::CubicBezier {
                    control_points: [p0, p1, p2, p3],
                })
            }
            other => Err(self.error(format!(
                "Unknown interpolation type {}",
                other.unwrap_or("undefined")
            ))),
        }
    }
}

fn is_interpolatable_type(space: ColorSpace, output: &Type) -> bool {
    // Arrays and composite types interpolate item by item; non-RGB spaces accept colours only.
    if space != ColorSpace::Rgb {
        return matches!(output, Type::Color | Type::ColorArray);
    }
    matches!(
        output,
        Type::Number
            | Type::Color
            | Type::Padding
            | Type::NumberArray
            | Type::ColorArray
            | Type::ProjectionDefinition
            | Type::VariableAnchorOffsetCollection
    ) || matches!(output, Type::Array { item, length: Some(_) } if **item == Type::Number)
}

impl Parser {
    /// Parses `label, output` pairs starting at `first_index`; a `step`'s first label is minus
    /// infinity and has no label element.
    fn parse_stops(
        &mut self,
        pairs: &[Json],
        first_index: usize,
        operator: &str,
        expected: Option<Type>,
    ) -> Result<(Vec<(f64, Expression)>, Type)> {
        let mut output = expected;
        let mut stops: Vec<(f64, Expression)> = Vec::new();
        let mut cursor = 0;
        let mut index = first_index;
        while cursor < pairs.len() {
            let Some(current) = pairs.get(cursor) else {
                break;
            };
            let (label, value) = if operator == "step" && stops.is_empty() {
                (f64::NEG_INFINITY, current)
            } else {
                let Some(label) = current.as_f64() else {
                    return Err(self.error(format!(
                        "Input/output pairs for \"{operator}\" expressions must be defined using literal numeric values (not computed expressions) for the input values."
                    )));
                };
                cursor += 1;
                index += 1;
                match pairs.get(cursor) {
                    Some(value) => (label, value),
                    None => return Err(self.error("Expected an even number of arguments.")),
                }
            };
            if stops.last().is_some_and(|(previous, _)| *previous >= label) {
                return Err(self.error(format!(
                    "Input/output pairs for \"{operator}\" expressions must be arranged with input values in strictly ascending order."
                )));
            }
            let parsed = self.parse_at(value, index, output.as_ref())?;
            if output.is_none() && parsed.output_type() != Type::Error {
                output = Some(parsed.output_type());
            }
            stops.push((label, parsed));
            cursor += 1;
            index += 1;
        }
        Ok((stops, output.unwrap_or(Type::Value)))
    }
}
