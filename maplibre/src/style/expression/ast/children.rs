//! The direct children of an expression node, which classification walks.

use super::Expression;

impl Expression {
    /// Calls `visit` on every direct child.
    pub fn for_each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a Expression)) {
        match self {
            Self::Literal(_)
            | Self::Folded { .. }
            | Self::Global(_)
            | Self::Feature(_)
            | Self::Accumulated
            | Self::Within(_)
            | Self::Distance(_)
            | Self::GlobalState(_) => {}
            Self::Coalesce { operands, .. }
            | Self::All(operands)
            | Self::Any(operands)
            | Self::Arithmetic { operands, .. }
            | Self::MinMax { operands, .. }
            | Self::Assert { operands, .. }
            | Self::Coerce { operands, .. }
            | Self::Rgba(operands)
            | Self::Concat(operands) => operands.iter().for_each(visit),
            Self::Not(operand)
            | Self::Error(operand)
            | Self::ResolvedLocale(operand)
            | Self::IsSupportedScript(operand)
            | Self::Length(operand)
            | Self::Math { operand, .. }
            | Self::TypeOf(operand)
            | Self::Image(operand)
            | Self::ToRgba(operand)
            | Self::StringCase { operand, .. } => visit(operand),
            Self::In {
                needle: left,
                haystack: right,
            }
            | Self::At {
                index: left,
                array: right,
                ..
            }
            | Self::Split {
                input: left,
                delimiter: right,
            }
            | Self::Join {
                input: left,
                separator: right,
            } => {
                visit(left);
                visit(right);
            }
            Self::Format(sections) => sections.iter().for_each(|section| section.for_each(visit)),
            other => for_each_structured_child(other, visit),
        }
    }
}

fn for_each_structured_child<'a>(expr: &'a Expression, visit: &mut dyn FnMut(&'a Expression)) {
    match expr {
        Expression::NumberFormat { input, options } => {
            visit(input);
            options.iter().for_each(|(_, option)| visit(option));
        }
        Expression::Get { key, object } | Expression::Has { key, object } => {
            visit(key);
            if let Some(object) = object {
                visit(object);
            }
        }
        Expression::Var { bound, .. } => visit(bound),
        Expression::Let { bindings, body } => {
            bindings.iter().for_each(|(_, bound)| visit(bound));
            visit(body);
        }
        Expression::Case {
            branches, fallback, ..
        } => {
            for (condition, output) in branches {
                visit(condition);
                visit(output);
            }
            visit(fallback);
        }
        Expression::Match {
            input,
            cases,
            fallback,
            ..
        } => {
            visit(input);
            cases.iter().for_each(|(_, output)| visit(output));
            visit(fallback);
        }
        Expression::Compare {
            left,
            right,
            collator,
            ..
        } => {
            visit(left);
            visit(right);
            if let Some(collator) = collator {
                visit(collator);
            }
        }
        Expression::Interpolate { input, stops, .. } | Expression::Step { input, stops, .. } => {
            visit(input);
            stops.iter().for_each(|(_, output)| visit(output));
        }
        other => for_each_string_and_collator_child(other, visit),
    }
}

fn for_each_string_and_collator_child<'a>(
    expr: &'a Expression,
    visit: &mut dyn FnMut(&'a Expression),
) {
    match expr {
        Expression::Collator {
            case_sensitive,
            diacritic_sensitive,
            locale,
        } => {
            visit(case_sensitive);
            visit(diacritic_sensitive);
            if let Some(locale) = locale {
                visit(locale);
            }
        }
        Expression::IndexOf {
            needle,
            haystack,
            from,
        } => {
            visit(needle);
            visit(haystack);
            if let Some(from) = from {
                visit(from);
            }
        }
        Expression::Slice {
            input, from, to, ..
        } => {
            visit(input);
            visit(from);
            if let Some(to) = to {
                visit(to);
            }
        }
        _ => {}
    }
}
