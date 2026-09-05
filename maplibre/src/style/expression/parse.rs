//! Parsing of the JSON form of an expression into an [`Expression`], with the type checks and
//! annotations GL JS applies while parsing.

use serde_json::Value as Json;
use thiserror::Error;

use super::{
    ast::{
        Arithmetic, Coercion, Expression, FeatureProperty, Global, MathFunction, StringFunction,
    },
    evaluate::EvaluationContext,
    value::{Type, Value},
};

mod compound;
mod curves;
mod operators;

pub use operators::is_expression;

/// Why an expression could not be parsed, with the path of the offending element.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
#[error("{key}: {message}")]
pub struct ParseError {
    /// Path of the element within the expression, such as `[2][1]`; empty at the root.
    pub key: String,
    /// What was wrong with it.
    pub message: String,
}

impl ParseError {
    /// Whether the expression uses an operator the engine does not implement.
    pub fn is_unknown_operator(&self) -> bool {
        self.message.starts_with("Unknown expression")
    }
}

pub(super) type Result<T> = std::result::Result<T, ParseError>;

/// How a parsed expression is fitted to the type its context expects.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Annotation {
    /// Wrap an untyped value in an assertion, a string in a colour coercion.
    Assert,
    /// Wrap an untyped value in a coercion, what GL JS does at the root of string properties.
    Coerce,
    /// Check the type but leave the expression bare; `coalesce` operands take this.
    Omit,
}

/// Parser state: the path being parsed and the variables in scope.
pub(super) struct Parser {
    pub(super) key: String,
    pub(super) scope: Vec<(String, Expression)>,
    /// Whether a `null` literal may stand where a typed value is expected. A lowered legacy
    /// function without a default falls back to `null`, which evaluates to nothing so the
    /// caller applies the property's own default; user-written expressions stay strict.
    null_fallbacks: bool,
}

impl Parser {
    pub(super) fn new() -> Self {
        Self {
            key: String::new(),
            scope: Vec::new(),
            null_fallbacks: false,
        }
    }

    pub(super) fn for_legacy_function() -> Self {
        Self {
            null_fallbacks: true,
            ..Self::new()
        }
    }

    pub(super) fn error(&self, message: impl Into<String>) -> ParseError {
        ParseError {
            key: self.key.clone(),
            message: message.into(),
        }
    }

    /// Parses the element at `index` of the current array.
    pub(super) fn parse_at(
        &mut self,
        json: &Json,
        index: usize,
        expected: Option<&Type>,
    ) -> Result<Expression> {
        let length = self.key.len();
        self.key.push_str(&format!("[{index}]"));
        let result = self.parse(json, expected);
        self.key.truncate(length);
        result
    }

    /// Parses `json`, wrapping it in the assertion or coercion `expected` calls for.
    pub(super) fn parse(&mut self, json: &Json, expected: Option<&Type>) -> Result<Expression> {
        self.parse_with(json, expected, Annotation::Assert)
    }

    /// Parses `json` with the given way of fitting it to `expected`.
    pub(super) fn parse_with(
        &mut self,
        json: &Json,
        expected: Option<&Type>,
        annotation: Annotation,
    ) -> Result<Expression> {
        let parsed = self.parse_bare(json, expected)?;
        let annotated = self.annotate(parsed, expected, annotation)?;
        self.fold(annotated)
    }

    /// Parses the element at `index` of the current array, checking its type without
    /// wrapping it.
    pub(super) fn parse_unwrapped_at(
        &mut self,
        json: &Json,
        index: usize,
        expected: Option<&Type>,
    ) -> Result<Expression> {
        let length = self.key.len();
        self.key.push_str(&format!("[{index}]"));
        let result = self.parse_with(json, expected, Annotation::Omit);
        self.key.truncate(length);
        result
    }

    fn parse_bare(&mut self, json: &Json, expected: Option<&Type>) -> Result<Expression> {
        match json {
            Json::Null | Json::Bool(_) | Json::Number(_) | Json::String(_) => {
                Ok(Expression::Literal(Value::from_json(json)))
            }
            Json::Object(_) => {
                Err(self.error("Bare objects invalid. Use [\"literal\", {...}] instead."))
            }
            Json::Array(items) => {
                let Some(first) = items.first() else {
                    return Err(self.error("Expected an array with at least one element."));
                };
                let Some(operator) = first.as_str() else {
                    return Err(self.error(format!(
                        "Expression name must be a string, but found {} instead. If you wanted a literal array, use [\"literal\", [...]].",
                        json_kind(first)
                    )));
                };
                self.parse_operator(operator, items, expected)
            }
        }
    }

    fn annotate(
        &self,
        parsed: Expression,
        expected: Option<&Type>,
        annotation: Annotation,
    ) -> Result<Expression> {
        let Some(expected) = expected else {
            return Ok(parsed);
        };
        let actual = parsed.output_type();
        let assertable = matches!(
            expected,
            Type::String | Type::Number | Type::Boolean | Type::Object | Type::Array { .. }
        );
        if assertable && actual == Type::Value {
            return Ok(match annotation {
                Annotation::Assert => Expression::Assert {
                    required: expected.clone(),
                    operands: vec![parsed],
                },
                Annotation::Coerce if *expected == Type::String => Expression::Coerce {
                    coercion: Coercion::String,
                    operands: vec![parsed],
                },
                Annotation::Coerce | Annotation::Omit => parsed,
            });
        }
        if *expected == Type::Color && matches!(actual, Type::Value | Type::String) {
            return Ok(match annotation {
                Annotation::Omit => parsed,
                _ => Expression::Coerce {
                    coercion: Coercion::Color,
                    operands: vec![parsed],
                },
            });
        }
        if self.null_fallbacks && actual == Type::Null {
            return Ok(parsed);
        }
        if !actual.is_subtype_of(expected) {
            return Err(self.error(format!("Expected {expected} but found {actual} instead.")));
        }
        Ok(parsed)
    }

    /// Evaluates an expression that depends on nothing, as GL JS does while parsing, so an
    /// error in it surfaces once rather than per feature.
    fn fold(&self, parsed: Expression) -> Result<Expression> {
        if matches!(parsed, Expression::Literal(_) | Expression::Folded { .. })
            || !parsed.is_constant()
        {
            return Ok(parsed);
        }
        let output = parsed.output_type();
        match parsed.evaluate(&EvaluationContext::default()) {
            Ok(value) => Ok(Expression::Folded { value, output }),
            Err(error) => Err(self.error(error.to_string())),
        }
    }

    fn parse_operator(
        &mut self,
        operator: &str,
        items: &[Json],
        expected: Option<&Type>,
    ) -> Result<Expression> {
        let args = &items[1..];
        // The expected type seeds the output type of branches and ramps; `value` says nothing.
        let output = expected.filter(|expected| **expected != Type::Value);
        match operator {
            "literal" => {
                if args.len() != 1 {
                    return Err(self.error(format!(
                        "'literal' expression requires exactly one argument, but found {} instead.",
                        args.len()
                    )));
                }
                Ok(Expression::Literal(Value::from_json(&args[0])))
            }
            "zoom" => self.nullary(args, Expression::Global(Global::Zoom)),
            "elevation" => self.nullary(args, Expression::Global(Global::Elevation)),
            "id" => self.nullary(args, Expression::Feature(FeatureProperty::Id)),
            "geometry-type" => self.nullary(args, Expression::Feature(FeatureProperty::GeometryType)),
            "properties" => self.nullary(args, Expression::Feature(FeatureProperty::Properties)),
            "pi" => self.nullary(args, Expression::Literal(Value::Number(std::f64::consts::PI))),
            "e" => self.nullary(args, Expression::Literal(Value::Number(std::f64::consts::E))),
            "ln2" => self.nullary(args, Expression::Literal(Value::Number(std::f64::consts::LN_2))),
            "get" | "has" => self.parse_lookup(operator, args),
            "global-state" => match args {
                [Json::String(key)] => Ok(Expression::GlobalState(key.clone())),
                _ => Err(self.error("Expected 1 argument, but found a different shape.")),
            },
            "var" => self.parse_var(args),
            "let" => self.parse_let(args, expected),
            "case" => self.parse_case(args, output),
            "match" => self.parse_match(args, output),
            "coalesce" => self.parse_coalesce(args, output),
            "step" => self.parse_step(args, output),
            "interpolate" | "interpolate-lab" | "interpolate-hcl" => {
                self.parse_interpolate(operator, args, output)
            }
            "==" | "!=" | "<" | "<=" | ">" | ">=" => self.parse_comparison(operator, args),
            "all" | "any" => {
                let operands = self.parse_varargs(args, &Type::Boolean)?;
                Ok(if operator == "all" {
                    Expression::All(operands)
                } else {
                    Expression::Any(operands)
                })
            }
            "!" => Ok(Expression::Not(Box::new(self.parse_single(args, &Type::Boolean)?))),
            "in" => self.parse_in(args),
            "index-of" => self.parse_index_of(args),
            "slice" => self.parse_slice(args),
            "length" => self.parse_length(args),
            "+" => self.parse_arithmetic(Arithmetic::Add, args, None),
            "*" => self.parse_arithmetic(Arithmetic::Multiply, args, None),
            "-" => match args.len() {
                1 | 2 => self.parse_arithmetic(Arithmetic::Subtract, args, None),
                found => Err(self.error(format!("Expected 1 or 2 arguments, but found {found} instead."))),
            },
            "/" => self.parse_arithmetic(Arithmetic::Divide, args, Some(2)),
            "%" => self.parse_arithmetic(Arithmetic::Remainder, args, Some(2)),
            "^" => self.parse_arithmetic(Arithmetic::Power, args, Some(2)),
            "sqrt" | "ln" | "log10" | "log2" | "sin" | "cos" | "tan" | "asin" | "acos" | "atan"
            | "abs" | "round" | "floor" | "ceil" => {
                let function = match operator {
                    "sqrt" => MathFunction::Sqrt,
                    "ln" => MathFunction::Ln,
                    "log10" => MathFunction::Log10,
                    "log2" => MathFunction::Log2,
                    "sin" => MathFunction::Sin,
                    "cos" => MathFunction::Cos,
                    "tan" => MathFunction::Tan,
                    "asin" => MathFunction::Asin,
                    "acos" => MathFunction::Acos,
                    "atan" => MathFunction::Atan,
                    "abs" => MathFunction::Abs,
                    "round" => MathFunction::Round,
                    "floor" => MathFunction::Floor,
                    _ => MathFunction::Ceil,
                };
                Ok(Expression::Math {
                    function,
                    operand: Box::new(self.parse_single(args, &Type::Number)?),
                })
            }
            "min" | "max" => Ok(Expression::MinMax {
                max: operator == "max",
                operands: self.parse_varargs(args, &Type::Number)?,
            }),
            "typeof" => Ok(Expression::TypeOf(Box::new(self.parse_single(args, &Type::Value)?))),
            "number" | "string" | "boolean" | "object" => {
                let required = match operator {
                    "number" => Type::Number,
                    "string" => Type::String,
                    "boolean" => Type::Boolean,
                    _ => Type::Object,
                };
                let operands = self.parse_at_least_one(args, &Type::Value)?;
                Ok(Expression::Assert { required, operands })
            }
            "array" => self.parse_array_assertion(args),
            "to-number" | "to-color" => {
                let operands = self.parse_at_least_one(args, &Type::Value)?;
                Ok(Expression::Coerce {
                    coercion: if operator == "to-number" {
                        Coercion::Number
                    } else {
                        Coercion::Color
                    },
                    operands,
                })
            }
            "to-string" | "to-boolean" => {
                if args.len() != 1 {
                    return Err(self.error("Expected one argument."));
                }
                let operands = vec![self.parse_at(&args[0], 1, Some(&Type::Value))?];
                Ok(Expression::Coerce {
                    coercion: if operator == "to-string" {
                        Coercion::String
                    } else {
                        Coercion::Boolean
                    },
                    operands,
                })
            }
            "to-rgba" => Ok(Expression::ToRgba(Box::new(self.parse_single(args, &Type::Color)?))),
            "rgb" => Ok(Expression::Rgba(self.parse_exactly(args, 3, &Type::Number)?)),
            "rgba" => Ok(Expression::Rgba(self.parse_exactly(args, 4, &Type::Number)?)),
            "concat" => Ok(Expression::Concat(self.parse_varargs(args, &Type::Value)?)),
            "upcase" | "downcase" => Ok(Expression::StringCase {
                function: if operator == "upcase" {
                    StringFunction::Upcase
                } else {
                    StringFunction::Downcase
                },
                operand: Box::new(self.parse_single(args, &Type::String)?),
            }),
            _ => Err(self.error(format!(
                "Unknown expression \"{operator}\". If you wanted a literal array, use [\"literal\", [...]]."
            ))),
        }
    }

    fn nullary(&self, args: &[Json], expression: Expression) -> Result<Expression> {
        if !args.is_empty() {
            return Err(self.error(format!(
                "Expected 0 arguments, but found {} instead.",
                args.len()
            )));
        }
        Ok(expression)
    }

    pub(super) fn parse_single(&mut self, args: &[Json], expected: &Type) -> Result<Expression> {
        if args.len() != 1 {
            return Err(self.error(format!(
                "Expected 1 argument, but found {} instead.",
                args.len()
            )));
        }
        self.parse_at(&args[0], 1, Some(expected))
    }

    pub(super) fn parse_exactly(
        &mut self,
        args: &[Json],
        count: usize,
        expected: &Type,
    ) -> Result<Vec<Expression>> {
        if args.len() != count {
            return Err(self.error(format!(
                "Expected {count} arguments, but found {} instead.",
                args.len()
            )));
        }
        self.parse_varargs(args, expected)
    }

    fn parse_at_least_one(&mut self, args: &[Json], expected: &Type) -> Result<Vec<Expression>> {
        if args.is_empty() {
            return Err(self.error("Expected at least one argument."));
        }
        self.parse_varargs(args, expected)
    }

    pub(super) fn parse_varargs(
        &mut self,
        args: &[Json],
        expected: &Type,
    ) -> Result<Vec<Expression>> {
        args.iter()
            .enumerate()
            .map(|(offset, arg)| self.parse_at(arg, offset + 1, Some(expected)))
            .collect()
    }
}

/// The JSON kind name GL JS uses in its messages.
pub(super) fn json_kind(json: &Json) -> &'static str {
    match json {
        Json::Null => "null",
        Json::Bool(_) => "boolean",
        Json::Number(_) => "number",
        Json::String(_) => "string",
        Json::Array(_) => "array",
        Json::Object(_) => "object",
    }
}
