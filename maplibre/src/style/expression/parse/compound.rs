//! Parsing of the compound operators: lookups, variables, comparisons, searches, arithmetic
//! and the array assertion.

use serde_json::Value as Json;

use super::{Parser, Result};
use crate::style::expression::{
    ast::{Arithmetic, Comparison, Expression},
    value::Type,
};

impl Parser {
    pub(super) fn parse_lookup(&mut self, operator: &str, args: &[Json]) -> Result<Expression> {
        if args.is_empty() || args.len() > 2 {
            return Err(self.error(format!(
                "Expected 1 or 2 arguments, but found {} instead.",
                args.len()
            )));
        }
        let key = Box::new(self.parse_at(&args[0], 1, Some(&Type::String))?);
        let object = match args.get(1) {
            Some(object) => Some(Box::new(self.parse_at(object, 2, Some(&Type::Object))?)),
            None => None,
        };
        Ok(if operator == "get" {
            Expression::Get { key, object }
        } else {
            Expression::Has { key, object }
        })
    }

    pub(super) fn parse_var(&mut self, args: &[Json]) -> Result<Expression> {
        let name = match args {
            [Json::String(name)] => name.clone(),
            _ => {
                return Err(
                    self.error("'var' expression requires exactly one string literal argument.")
                )
            }
        };
        match self.scope.iter().rev().find(|(bound, _)| *bound == name) {
            Some((_, bound)) => Ok(Expression::Var {
                name,
                bound: Box::new(bound.clone()),
            }),
            None => Err(self.error(format!(
                "Unknown variable \"{name}\". Make sure \"{name}\" has been bound in an enclosing \"let\" expression before using it."
            ))),
        }
    }

    pub(super) fn with_bindings<T>(
        &mut self,
        bindings: &[(String, Expression)],
        parse: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        let depth = self.scope.len();
        self.scope.extend(bindings.iter().cloned());
        let result = parse(self);
        self.scope.truncate(depth);
        result
    }

    pub(super) fn parse_comparison(&mut self, operator: &str, args: &[Json]) -> Result<Expression> {
        let operator_kind = match operator {
            "==" => Comparison::Equal,
            "!=" => Comparison::NotEqual,
            "<" => Comparison::Less,
            "<=" => Comparison::LessEqual,
            ">" => Comparison::Greater,
            _ => Comparison::GreaterEqual,
        };
        if args.len() != 2 && args.len() != 3 {
            return Err(self.error("Expected two or three arguments."));
        }
        if args.len() == 3 {
            return Err(self.error("Unknown expression \"collator\"."));
        }
        let mut left = self.parse_at(&args[0], 1, Some(&Type::Value))?;
        let mut right = self.parse_at(&args[1], 2, Some(&Type::Value))?;
        let (left_type, right_type) = (left.output_type(), right.output_type());
        for (index, side) in [(1, &left_type), (2, &right_type)] {
            let comparable = match operator_kind.is_ordering() {
                true => matches!(side, Type::String | Type::Number | Type::Value),
                false => matches!(
                    side,
                    Type::Boolean | Type::String | Type::Number | Type::Null | Type::Value
                ),
            };
            if !comparable {
                let length = self.key.len();
                self.key.push_str(&format!("[{index}]"));
                let error = self.error(format!(
                    "\"{operator}\" comparisons are not supported for type '{side}'."
                ));
                self.key.truncate(length);
                return Err(error);
            }
        }
        if left_type != right_type && left_type != Type::Value && right_type != Type::Value {
            return Err(self.error(format!(
                "Cannot compare types '{left_type}' and '{right_type}'."
            )));
        }
        if operator_kind.is_ordering() {
            if left_type == Type::Value && right_type != Type::Value {
                left = Expression::Assert {
                    required: right_type.clone(),
                    operands: vec![left],
                };
            } else if left_type != Type::Value && right_type == Type::Value {
                right = Expression::Assert {
                    required: left_type.clone(),
                    operands: vec![right],
                };
            }
        }
        Ok(Expression::Compare {
            operator: operator_kind,
            untyped: left_type == Type::Value || right_type == Type::Value,
            left: Box::new(left),
            right: Box::new(right),
        })
    }

    pub(super) fn parse_in(&mut self, args: &[Json]) -> Result<Expression> {
        if args.len() != 2 {
            return Err(self.error(format!(
                "Expected 2 arguments, but found {} instead.",
                args.len()
            )));
        }
        let needle = self.parse_at(&args[0], 1, Some(&Type::Value))?;
        let haystack = self.parse_at(&args[1], 2, Some(&Type::Value))?;
        self.check_searchable(&needle)?;
        Ok(Expression::In {
            needle: Box::new(needle),
            haystack: Box::new(haystack),
        })
    }

    fn check_searchable(&self, needle: &Expression) -> Result<()> {
        let needle_type = needle.output_type();
        if !matches!(
            needle_type,
            Type::Boolean | Type::String | Type::Number | Type::Null | Type::Value
        ) {
            return Err(self.error(format!(
                "Expected first argument to be of type boolean, string, number or null, but found {needle_type} instead"
            )));
        }
        Ok(())
    }

    pub(super) fn parse_index_of(&mut self, args: &[Json]) -> Result<Expression> {
        if args.len() != 2 && args.len() != 3 {
            return Err(self.error(format!(
                "Expected 2 or 3 arguments, but found {} instead.",
                args.len()
            )));
        }
        let needle = self.parse_at(&args[0], 1, Some(&Type::Value))?;
        let haystack = self.parse_at(&args[1], 2, Some(&Type::Value))?;
        self.check_searchable(&needle)?;
        let from = match args.get(2) {
            Some(from) => Some(Box::new(self.parse_at(from, 3, Some(&Type::Number))?)),
            None => None,
        };
        Ok(Expression::IndexOf {
            needle: Box::new(needle),
            haystack: Box::new(haystack),
            from,
        })
    }

    pub(super) fn parse_slice(&mut self, args: &[Json]) -> Result<Expression> {
        if args.len() != 2 && args.len() != 3 {
            return Err(self.error(format!(
                "Expected 2 or 3 arguments, but found {} instead.",
                args.len()
            )));
        }
        let input = self.parse_at(&args[0], 1, Some(&Type::Value))?;
        let from = self.parse_at(&args[1], 2, Some(&Type::Number))?;
        let output = input.output_type();
        if !matches!(output, Type::String | Type::Value | Type::Array { .. }) {
            return Err(self.error(format!(
                "Expected first argument to be of type array or string, but found {output} instead"
            )));
        }
        let to = match args.get(2) {
            Some(to) => Some(Box::new(self.parse_at(to, 3, Some(&Type::Number))?)),
            None => None,
        };
        Ok(Expression::Slice {
            input: Box::new(input),
            from: Box::new(from),
            to,
            output,
        })
    }

    pub(super) fn parse_length(&mut self, args: &[Json]) -> Result<Expression> {
        if args.len() != 1 {
            return Err(self.error(format!(
                "Expected 1 argument, but found {} instead.",
                args.len()
            )));
        }
        let input = self.parse_at(&args[0], 1, Some(&Type::Value))?;
        let input_type = input.output_type();
        if !matches!(input_type, Type::String | Type::Value | Type::Array { .. }) {
            return Err(self.error(format!(
                "Expected argument of type string or array, but found {input_type} instead."
            )));
        }
        Ok(Expression::Length(Box::new(input)))
    }

    pub(super) fn parse_arithmetic(
        &mut self,
        operator: Arithmetic,
        args: &[Json],
        arity: Option<usize>,
    ) -> Result<Expression> {
        let operands = match arity {
            Some(count) => self.parse_exactly(args, count, &Type::Number)?,
            None => self.parse_varargs(args, &Type::Number)?,
        };
        Ok(Expression::Arithmetic { operator, operands })
    }

    pub(super) fn parse_array_assertion(&mut self, args: &[Json]) -> Result<Expression> {
        if args.is_empty() {
            return Err(self.error("Expected at least one argument."));
        }
        let mut first_value = 0;
        let mut item = Type::Value;
        let mut length = None;
        if args.len() > 1 {
            item = match args[0].as_str() {
                Some("string") => Type::String,
                Some("number") => Type::Number,
                Some("boolean") => Type::Boolean,
                _ => return Err(self.error(
                    "The item type argument of \"array\" must be one of string, number, boolean",
                )),
            };
            first_value = 1;
        }
        if args.len() > 2 {
            length = match &args[1] {
                Json::Null => None,
                Json::Number(number) => match number.as_u64() {
                    Some(count) if number.as_f64() == Some(count as f64) => Some(count as usize),
                    _ => {
                        return Err(self.error(
                            "The length argument to \"array\" must be a positive integer literal",
                        ))
                    }
                },
                _ => {
                    return Err(self.error(
                        "The length argument to \"array\" must be a positive integer literal",
                    ))
                }
            };
            first_value = 2;
        }
        let operands = args[first_value..]
            .iter()
            .enumerate()
            .map(|(offset, arg)| self.parse_at(arg, first_value + offset + 1, Some(&Type::Value)))
            .collect::<Result<Vec<_>>>()?;
        Ok(Expression::Assert {
            required: Type::array(item, length),
            operands,
        })
    }
}
