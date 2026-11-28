//! Evaluation of the expressions that make images and formatted text, with the GL JS rules for
//! an image the map does not hold.

use super::{EvaluationContext, Result};
use crate::style::expression::{
    ast::{Expression, FormatSectionNode},
    value::{Formatted, FormattedSection, ResolvedImage, Value},
};

impl Expression {
    /// The image `name` evaluates to, available when the context's images hold it. An empty or
    /// null name is no image.
    pub(super) fn eval_image(name: &Self, context: &EvaluationContext) -> Result<Value> {
        let name = name.evaluate(context)?.to_display_string();
        Ok(resolved(name, context))
    }

    /// The first operand that is not null and not an image the map lacks. When the last operand
    /// is such an image too, the name of the first image asked for, so the map still requests it.
    pub(super) fn eval_coalesce(operands: &[Self], context: &EvaluationContext) -> Result<Value> {
        let mut requested: Option<String> = None;
        for (index, operand) in operands.iter().enumerate() {
            let value = operand.evaluate(context)?;
            match value {
                Value::Image(ResolvedImage {
                    name,
                    available: false,
                }) => {
                    let first = requested.get_or_insert(name);
                    if index + 1 == operands.len() {
                        return Ok(Value::String(first.clone()));
                    }
                }
                Value::Null => {}
                value => return Ok(value),
            }
        }
        Ok(Value::Null)
    }

    fn eval_optional(node: Option<&Self>, context: &EvaluationContext) -> Result<Option<Value>> {
        node.map(|expr| expr.evaluate(context)).transpose()
    }

    fn eval_format_section(
        section: &FormatSectionNode<Self>,
        context: &EvaluationContext,
    ) -> Result<FormattedSection> {
        let vertical_align = Self::eval_optional(section.vertical_align.as_ref(), context)?
            .and_then(|value| value.as_str().map(str::to_owned));
        let content = section.content.evaluate(context)?;
        if let Value::Image(image) = content {
            return Ok(FormattedSection {
                image: Some(image),
                vertical_align,
                ..FormattedSection::default()
            });
        }
        let scale =
            Self::eval_optional(section.scale.as_ref(), context)?.and_then(|v| v.as_number());
        let font = Self::eval_optional(section.font.as_ref(), context)?.and_then(|val| match val {
            Value::Array(fonts) => Some(fonts.iter().map(Value::to_display_string).collect()),
            _ => None,
        });
        let color =
            Self::eval_optional(section.color.as_ref(), context)?.and_then(|val| match val {
                Value::Color(color) => Some(color),
                _ => None,
            });
        Ok(FormattedSection {
            text: content.to_display_string(),
            image: None,
            scale,
            font,
            color,
            vertical_align,
        })
    }

    /// The sections of a `format` expression. A section whose content is an image is that image,
    /// keeping only its vertical alignment; any other content becomes its text.
    pub(super) fn eval_format(
        sections: &[FormatSectionNode<Self>],
        context: &EvaluationContext,
    ) -> Result<Value> {
        let formatted = sections
            .iter()
            .map(|section| Self::eval_format_section(section, context))
            .collect::<Result<Vec<_>>>()?;
        Ok(Value::Formatted(Formatted {
            sections: formatted,
        }))
    }
}

fn resolved(name: String, context: &EvaluationContext) -> Value {
    if name.is_empty() {
        return Value::Null;
    }
    let available = context
        .available_images
        .is_some_and(|images| images.contains_image(&name));
    Value::Image(ResolvedImage { name, available })
}

/// `value` as formatted text: formatted text stays as it is, an image becomes one image
/// section and anything else one section of its text.
pub(super) fn to_formatted(value: Value) -> Value {
    Value::Formatted(match value {
        Value::Formatted(formatted) => formatted,
        Value::Image(image) => Formatted::image(image),
        other => Formatted::plain(other.to_display_string()),
    })
}

/// `value` as an image, by its text.
pub(super) fn to_image(value: Value, context: &EvaluationContext) -> Value {
    match value {
        Value::Image(image) => Value::Image(image),
        other => resolved(other.to_display_string(), context),
    }
}

#[cfg(test)]
mod tests;
