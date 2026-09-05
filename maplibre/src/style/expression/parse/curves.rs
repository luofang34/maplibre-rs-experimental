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
        let mut bindings = Vec::new();
        for (offset, pair) in args[..args.len() - 1].chunks(2).enumerate() {
            let index = offset * 2 + 1;
            let Some(name) = pair[0].as_str() else {
                return Err(self.error(format!(
                    "Expected string, but found {} instead.",
                    json_kind(&pair[0])
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
        let last = args.len() - 1;
        let body = self.with_bindings(&bindings, |parser| {
            parser.parse_at(&args[last], last + 1, expected)
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
        let mut output: Option<Type> = expected.cloned();
        let mut branches = Vec::new();
        for (offset, pair) in args[..args.len() - 1].chunks(2).enumerate() {
            let index = offset * 2 + 1;
            let condition = self.parse_at(&pair[0], index, Some(&Type::Boolean))?;
            let result = self.parse_at(&pair[1], index + 1, output.as_ref())?;
            output.get_or_insert_with(|| result.output_type());
            branches.push((condition, result));
        }
        let last = args.len() - 1;
        let fallback = self.parse_at(&args[last], last + 1, output.as_ref())?;
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
        for (offset, pair) in args[1..args.len() - 1].chunks(2).enumerate() {
            let index = offset * 2 + 2;
            let labels: Vec<&Json> = match &pair[0] {
                Json::Array(labels) => labels.iter().collect(),
                label => vec![label],
            };
            if labels.is_empty() {
                return Err(self.error("Expected at least one branch label."));
            }
            let mut values = Vec::new();
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
                match &input_type {
                    None => input_type = Some(value.type_of()),
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
            let result = self.parse_at(&pair[1], index + 1, output.as_ref())?;
            output.get_or_insert_with(|| result.output_type());
            cases.push((values, result));
        }
        let input = self.parse_at(&args[0], 1, Some(&Type::Value))?;
        let last = args.len() - 1;
        let fallback = self.parse_at(&args[last], last + 1, output.as_ref())?;
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
            output.get_or_insert_with(|| operand.output_type());
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
        let input = self.parse_at(&args[0], 1, Some(&Type::Number))?;
        let (stops, output) = self.parse_stops(&args[1..], 2, "step", expected.cloned())?;
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
        let interpolation = match curve.first().and_then(Json::as_str) {
            Some("linear") => Interpolation::Linear,
            Some("exponential") => match curve.get(1).and_then(Json::as_f64) {
                Some(base) => Interpolation::Exponential { base },
                None => {
                    return Err(self.error("Exponential interpolation requires a numeric base."))
                }
            },
            Some("cubic-bezier") => {
                let points: Vec<f64> = curve[1..].iter().filter_map(Json::as_f64).collect();
                if points.len() != 4
                    || curve.len() != 5
                    || points.iter().any(|point| !(0.0..=1.0).contains(point))
                {
                    return Err(self.error(
                        "Cubic bezier interpolation requires four numeric arguments with values between 0 and 1.",
                    ));
                }
                Interpolation::CubicBezier {
                    control_points: [points[0], points[1], points[2], points[3]],
                }
            }
            other => {
                return Err(self.error(format!(
                    "Unknown interpolation type {}",
                    other.unwrap_or("undefined")
                )))
            }
        };
        if args.len() < 4 {
            return Err(self.error(format!(
                "Expected at least 4 arguments, but found only {}.",
                args.len()
            )));
        }
        if !args.len().is_multiple_of(2) {
            return Err(self.error("Expected an even number of arguments."));
        }
        let input = self.parse_at(&args[1], 2, Some(&Type::Number))?;
        let space = match operator {
            "interpolate-lab" => ColorSpace::Lab,
            "interpolate-hcl" => ColorSpace::Hcl,
            _ => ColorSpace::Rgb,
        };
        let seed = if space != ColorSpace::Rgb {
            Some(Type::Color)
        } else {
            expected.cloned()
        };
        let (stops, output) = self.parse_stops(&args[2..], 3, "interpolate", seed)?;
        // Arrays interpolate item by item, which needs a length both stops share.
        let interpolatable = output == Type::Number
            || output == Type::Color
            || matches!(&output, Type::Array { item, length: Some(_) } if **item == Type::Number);
        if !interpolatable {
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
            let (label, value) = if operator == "step" && stops.is_empty() {
                (f64::NEG_INFINITY, &pairs[cursor])
            } else {
                let Some(label) = pairs[cursor].as_f64() else {
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
            output.get_or_insert_with(|| parsed.output_type());
            stops.push((label, parsed));
            cursor += 1;
            index += 1;
        }
        Ok((stops, output.unwrap_or(Type::Value)))
    }
}
