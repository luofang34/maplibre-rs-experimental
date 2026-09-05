//! Parsing of the legacy and expression filter syntaxes into a [`Filter`] tree.

use serde_json::Value;

use super::{Comparison, Filter, FilterError, Operand};

/// GL JS `isExpressionFilter`: whether an array filter uses expression rather than legacy syntax.
pub(super) fn is_expression_filter(items: &[Value]) -> bool {
    let Some(operator) = items.first().and_then(Value::as_str) else {
        return true;
    };
    match operator {
        "has" => items.len() >= 2 && !matches!(items[1].as_str(), Some("$id" | "$type")),
        "in" => items.len() >= 3 && (!items[1].is_string() || items[2].is_array()),
        "!in" | "!has" | "none" => false,
        "==" | "!=" | ">" | ">=" | "<" | "<=" => {
            items.len() != 3 || items[1].is_array() || items[2].is_array()
        }
        "any" | "all" => items[1..].iter().all(|item| {
            item.is_boolean()
                || item
                    .as_array()
                    .is_some_and(|nested| is_expression_filter(nested))
        }),
        _ => true,
    }
}

pub(super) fn parse_legacy(items: &[Value]) -> Result<Filter, FilterError> {
    let operator = items[0].as_str().unwrap_or_default();
    if items.len() <= 1 {
        return Ok(Filter::Literal(operator != "any"));
    }
    let malformed = |expected| FilterError::Malformed {
        operator: operator.to_string(),
        expected,
    };
    let key = || {
        items[1]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| malformed("a property name"))
    };
    let nested = || {
        items[1..]
            .iter()
            .map(Filter::parse)
            .collect::<Result<Vec<_>, _>>()
    };
    match operator {
        "all" => Ok(Filter::All(nested()?)),
        "any" => Ok(Filter::Any(nested()?)),
        "none" => Ok(Filter::Not(Box::new(Filter::Any(nested()?)))),
        "==" | "!=" | "<" | "<=" | ">" | ">=" => {
            let value = items
                .get(2)
                .ok_or_else(|| malformed("a property and a value"))?;
            let operator = Comparison::parse(operator).unwrap_or(Comparison::Equal);
            Ok(Filter::Compare {
                operator,
                left: legacy_operand(&key()?),
                right: Operand::Literal(value.clone()),
            })
        }
        "in" | "!in" => {
            let key = key()?;
            let values = items[2..].to_vec();
            let filter = if values.is_empty() {
                Filter::Literal(false)
            } else {
                Filter::In {
                    needle: legacy_operand(&key),
                    haystack: Operand::Literal(Value::Array(values)),
                }
            };
            Ok(if operator == "in" {
                filter
            } else {
                Filter::Not(Box::new(filter))
            })
        }
        "has" | "!has" => {
            let key = key()?;
            let filter = match key.as_str() {
                "$type" => Filter::Literal(true),
                "$id" => Filter::HasId,
                _ => Filter::Has(key),
            };
            Ok(if operator == "has" {
                filter
            } else {
                Filter::Not(Box::new(filter))
            })
        }
        _ => Err(FilterError::UnsupportedOperator {
            operator: operator.to_string(),
        }),
    }
}

fn legacy_operand(key: &str) -> Operand {
    match key {
        "$type" => Operand::GeometryType,
        "$id" => Operand::Id,
        _ => Operand::Get(key.to_string()),
    }
}

pub(super) fn parse_expression(items: &[Value]) -> Result<Filter, FilterError> {
    let Some(operator) = items[0].as_str() else {
        return Err(FilterError::NotAFilter {
            found: kind_name(&items[0]).to_string(),
        });
    };
    let malformed = |expected| FilterError::Malformed {
        operator: operator.to_string(),
        expected,
    };
    let argument = |index: usize, expected| items.get(index).ok_or_else(|| malformed(expected));
    let nested = || {
        items[1..]
            .iter()
            .map(Filter::parse)
            .collect::<Result<Vec<_>, _>>()
    };
    match operator {
        "all" => Ok(Filter::All(nested()?)),
        "any" => Ok(Filter::Any(nested()?)),
        "!" => Ok(Filter::Not(Box::new(Filter::parse(argument(
            1,
            "one filter",
        )?)?))),
        "==" | "!=" | "<" | "<=" | ">" | ">=" => parse_comparison(operator, items),
        "has" => match items {
            [_, Value::String(key)] => Ok(Filter::Has(key.clone())),
            _ => Err(malformed("one property name")),
        },
        "in" => match items {
            [_, needle, haystack] => Ok(Filter::In {
                needle: parse_operand(needle)?,
                haystack: parse_operand(haystack)?,
            }),
            _ => Err(malformed("a needle and a haystack")),
        },
        "match" => parse_match(items),
        "case" => parse_case(items),
        "literal" | "boolean" => match argument(1, "one boolean")? {
            Value::Bool(pass) => Ok(Filter::Literal(*pass)),
            _ => Err(malformed("one boolean")),
        },
        _ => Err(FilterError::UnsupportedOperator {
            operator: operator.to_string(),
        }),
    }
}

fn parse_comparison(operator: &str, items: &[Value]) -> Result<Filter, FilterError> {
    let [_, left, right] = items else {
        return Err(FilterError::Malformed {
            operator: operator.to_string(),
            expected: "two operands",
        });
    };
    Ok(Filter::Compare {
        operator: Comparison::parse(operator).unwrap_or(Comparison::Equal),
        left: parse_operand(left)?,
        right: parse_operand(right)?,
    })
}

/// Splits the arguments of `match` and `case` into their pairs and the trailing fallback.
fn pairs_and_fallback<'a>(
    operator: &str,
    rest: &'a [Value],
    expected: &'static str,
) -> Result<(&'a [Value], &'a Value), FilterError> {
    match rest.split_last() {
        Some((fallback, pairs)) if !pairs.is_empty() && pairs.len().is_multiple_of(2) => {
            Ok((pairs, fallback))
        }
        _ => Err(FilterError::Malformed {
            operator: operator.to_string(),
            expected,
        }),
    }
}

fn parse_match(items: &[Value]) -> Result<Filter, FilterError> {
    const EXPECTED: &str = "an input, label/output pairs and a fallback";
    let input = items.get(1).ok_or_else(|| FilterError::Malformed {
        operator: "match".to_string(),
        expected: EXPECTED,
    })?;
    let input = parse_operand(input)?;
    let (pairs, fallback) = pairs_and_fallback("match", &items[2..], EXPECTED)?;
    let cases = pairs
        .chunks(2)
        .map(|pair| {
            let labels = match &pair[0] {
                Value::Array(labels) => labels.clone(),
                label => vec![label.clone()],
            };
            Ok((labels, Filter::parse(&pair[1])?))
        })
        .collect::<Result<Vec<_>, FilterError>>()?;
    Ok(Filter::Match {
        input,
        cases,
        fallback: Box::new(Filter::parse(fallback)?),
    })
}

fn parse_case(items: &[Value]) -> Result<Filter, FilterError> {
    let (pairs, fallback) =
        pairs_and_fallback("case", &items[1..], "condition/output pairs and a fallback")?;
    let branches = pairs
        .chunks(2)
        .map(|pair| Ok((Filter::parse(&pair[0])?, Filter::parse(&pair[1])?)))
        .collect::<Result<Vec<_>, FilterError>>()?;
    Ok(Filter::Case {
        branches,
        fallback: Box::new(Filter::parse(fallback)?),
    })
}

fn parse_operand(value: &Value) -> Result<Operand, FilterError> {
    let items = match value {
        Value::Array(items) => items,
        Value::Object(_) => {
            return Err(FilterError::NotAFilter {
                found: "object".to_string(),
            })
        }
        literal => return Ok(Operand::Literal(literal.clone())),
    };
    let operator =
        items
            .first()
            .and_then(Value::as_str)
            .ok_or_else(|| FilterError::NotAFilter {
                found: "array without an operator".to_string(),
            })?;
    let malformed = |expected| FilterError::Malformed {
        operator: operator.to_string(),
        expected,
    };
    match operator {
        "get" => match items.as_slice() {
            [_, Value::String(key)] => Ok(Operand::Get(key.clone())),
            _ => Err(malformed("one property name")),
        },
        "geometry-type" => Ok(Operand::GeometryType),
        "id" => Ok(Operand::Id),
        "zoom" => Ok(Operand::Zoom),
        "literal" => items
            .get(1)
            .map(|value| Operand::Literal(value.clone()))
            .ok_or_else(|| malformed("one value")),
        "to-number" | "to-string" | "number" | "string" => {
            if items.len() != 2 {
                return Err(malformed("one operand"));
            }
            let inner = parse_operand(&items[1])?;
            Ok(match operator {
                "to-number" => Operand::ToNumber(Box::new(inner)),
                "to-string" => Operand::ToString(Box::new(inner)),
                _ => inner,
            })
        }
        _ => Err(FilterError::UnsupportedOperator {
            operator: operator.to_string(),
        }),
    }
}

pub(super) fn kind_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}
