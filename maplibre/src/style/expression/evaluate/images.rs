//! Evaluation of the expressions that make images and formatted text, with the GL JS rules for
//! an image the map does not hold.

use super::{EvaluationContext, Result};
use crate::style::expression::{
    ast::{Expression, FormatSection},
    value::{Formatted, FormattedSection, ResolvedImage, Value},
};

/// The image `name` evaluates to, available when the context's images hold it. An empty or
/// null name is no image.
pub(super) fn image(name: &Expression, context: &EvaluationContext) -> Result<Value> {
    let name = name.evaluate(context)?.to_display_string();
    Ok(resolved(name, context))
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

/// The first operand that is not null and not an image the map lacks. When the last operand is
/// such an image too, the name of the first image asked for, so the map still requests it.
pub(super) fn coalesce(operands: &[Expression], context: &EvaluationContext) -> Result<Value> {
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

/// The sections of a `format` expression. A section whose content is an image is that image,
/// keeping only its vertical alignment; any other content becomes its text.
pub(super) fn format(sections: &[FormatSection], context: &EvaluationContext) -> Result<Value> {
    let mut formatted = Formatted::default();
    for section in sections {
        let vertical_align = section
            .vertical_align
            .as_ref()
            .map(|align| align.evaluate(context))
            .transpose()?
            .and_then(|align| align.as_str().map(str::to_owned));
        let content = section.content.evaluate(context)?;
        if let Value::Image(image) = content {
            formatted.sections.push(FormattedSection {
                image: Some(image),
                vertical_align,
                ..FormattedSection::default()
            });
            continue;
        }
        let scale = section
            .scale
            .as_ref()
            .map(|scale| scale.evaluate(context))
            .transpose()?
            .and_then(|scale| scale.as_number());
        let font = section
            .font
            .as_ref()
            .map(|font| font.evaluate(context))
            .transpose()?
            .and_then(|font| match font {
                Value::Array(fonts) => Some(fonts.iter().map(Value::to_display_string).collect()),
                _ => None,
            });
        let color = section
            .color
            .as_ref()
            .map(|color| color.evaluate(context))
            .transpose()?
            .and_then(|color| match color {
                Value::Color(color) => Some(color),
                _ => None,
            });
        formatted.sections.push(FormattedSection {
            text: content.to_display_string(),
            image: None,
            scale,
            font,
            color,
            vertical_align,
        });
    }
    Ok(Value::Formatted(formatted))
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
