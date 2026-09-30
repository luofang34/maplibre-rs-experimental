//! The colour ramp a `line-gradient` is drawn through.

use csscolorparser::Color;

use crate::style::{
    expression::{Color as ExpressionColor, EvaluationContext, Value},
    layer::StyleProperty,
};

/// Texels in the ramp: the colour at each of this many positions from the start to the end of
/// a line.
pub const RAMP_TEXELS: usize = 256;

/// The colour of each of [`RAMP_TEXELS`] positions along the line, straight-alpha 8-bit RGBA.
/// A position whose colour cannot be evaluated is transparent.
pub fn ramp(gradient: &StyleProperty<Color>) -> Vec<[u8; 4]> {
    (0..RAMP_TEXELS)
        .map(|texel| {
            let progress = texel as f64 / (RAMP_TEXELS - 1) as f64;
            let color = match gradient {
                StyleProperty::Constant(color) => Some(ExpressionColor::from(color.clone())),
                StyleProperty::Expression(property) => {
                    let context = EvaluationContext {
                        line_progress: progress,
                        ..EvaluationContext::default()
                    };
                    match property.expression().evaluate(&context) {
                        Ok(Value::Color(color)) => Some(color),
                        _ => None,
                    }
                }
                StyleProperty::Unsupported(_) => None,
            };
            let [r, g, b, a] = color.map_or([0.0; 4], |color| color.straight());
            [r, g, b, a].map(|channel| (channel * 255.0).round() as u8)
        })
        .collect()
}

#[cfg(test)]
mod tests;
