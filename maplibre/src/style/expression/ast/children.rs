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
            | Self::Within(_)
            | Self::Distance(_)
            | Self::GlobalState(_) => {}
            Self::NumberFormat { input, options } => {
                visit(input);
                options.iter().for_each(|(_, option)| visit(option));
            }
            Self::Get { key, object } | Self::Has { key, object } => {
                visit(key);
                if let Some(object) = object {
                    visit(object);
                }
            }
            Self::Var { bound, .. } => visit(bound),
            Self::Let { bindings, body } => {
                for (_, bound) in bindings {
                    visit(bound);
                }
                visit(body);
            }
            Self::Case {
                branches, fallback, ..
            } => {
                for (condition, output) in branches {
                    visit(condition);
                    visit(output);
                }
                visit(fallback);
            }
            Self::Match {
                input,
                cases,
                fallback,
                ..
            } => {
                visit(input);
                for (_, output) in cases {
                    visit(output);
                }
                visit(fallback);
            }
            Self::Coalesce { operands, .. }
            | Self::All(operands)
            | Self::Any(operands)
            | Self::Arithmetic { operands, .. }
            | Self::MinMax { operands, .. }
            | Self::Assert { operands, .. }
            | Self::Coerce { operands, .. }
            | Self::Rgba(operands)
            | Self::Concat(operands) => operands.iter().for_each(visit),
            Self::Compare {
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
            Self::Collator {
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
            Self::Not(operand)
            | Self::ResolvedLocale(operand)
            | Self::IsSupportedScript(operand)
            | Self::Length(operand)
            | Self::Math { operand, .. }
            | Self::TypeOf(operand)
            | Self::Image(operand)
            | Self::ToRgba(operand)
            | Self::StringCase { operand, .. } => visit(operand),
            Self::In { needle, haystack } => {
                visit(needle);
                visit(haystack);
            }
            Self::IndexOf {
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
            Self::Slice {
                input, from, to, ..
            } => {
                visit(input);
                visit(from);
                if let Some(to) = to {
                    visit(to);
                }
            }
            Self::Interpolate { input, stops, .. } | Self::Step { input, stops, .. } => {
                visit(input);
                for (_, output) in stops {
                    visit(output);
                }
            }
            Self::Format(sections) => sections.iter().for_each(|section| section.for_each(visit)),
        }
    }
}
