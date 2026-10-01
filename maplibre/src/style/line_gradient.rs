//! The colour ramp a `line-gradient` is drawn through.

use csscolorparser::Color;

use crate::style::{
    expression::{Color as ExpressionColor, EvaluationContext, Value},
    layer::StyleProperty,
};

/// Texels in the ramp: the colour at each of this many positions from the start to the end of
/// a line.
pub const RAMP_TEXELS: usize = 256;

/// Texels in the ramp of a gradient made of steps, sampled without blending: wide enough that a
/// step as narrow as a thousandth of a line stays where it is, as GL JS widens it.
pub const STEP_RAMP_TEXELS: usize = 8192;

/// How far a stepped gradient's ramp reaches into the small progress values: texel `i` of `n`
/// holds the colour at `(2^(u * STEP_RAMP_SPAN_BITS) - 1) / 2^STEP_RAMP_SPAN_BITS` for
/// `u = i / (n - 1)`, so a step at a millionth of a very long line stays as sharp as one at a
/// tenth of a short one.
pub const STEP_RAMP_SPAN_BITS: f64 = 20.0;

/// The progress a position `u` in `0..=1` of a stepped gradient's ramp stands for.
pub fn stepped_progress(u: f64) -> f64 {
    ((u * STEP_RAMP_SPAN_BITS).exp2() - 1.0) / STEP_RAMP_SPAN_BITS.exp2()
}

/// Whether the gradient is a `step` expression, whose edges are hard.
pub fn is_stepped(gradient: &StyleProperty<Color>) -> bool {
    matches!(
        gradient.expression(),
        Some(crate::style::expression::Expression::Step { .. })
    )
}

/// The colour of each of [`RAMP_TEXELS`] positions along the line, or [`STEP_RAMP_TEXELS`] for
/// steps, straight-alpha 8-bit RGBA. A position whose colour cannot be evaluated is transparent.
pub fn ramp(gradient: &StyleProperty<Color>) -> Vec<[u8; 4]> {
    let texels = if is_stepped(gradient) {
        STEP_RAMP_TEXELS
    } else {
        RAMP_TEXELS
    };
    (0..texels)
        .map(|texel| {
            let position = texel as f64 / (texels - 1) as f64;
            let progress = if texels == STEP_RAMP_TEXELS {
                stepped_progress(position)
            } else {
                position
            };
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
