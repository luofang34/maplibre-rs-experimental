//! The colour ramp a `line-gradient` is drawn through.

use csscolorparser::Color;

use crate::style::{
    expression::{Color as ExpressionColor, EvaluationContext, Value},
    layer::StyleProperty,
};

/// Texels in the ramp: the colour at each of this many positions from the start to the end of
/// a line.
pub const RAMP_TEXELS: usize = 256;

/// High-resolution linear ramp used for nested `step`, `step` with stops away from `0.0`, and
/// narrow `interpolate` bands where 256 texels would quantize sharp transitions.
pub const HIGH_RES_RAMP_TEXELS: usize = 4096;

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

/// Whether the gradient uses the logarithmic stepped ramp (a top-level `step` with a stop near
/// `0.0`, where logarithmic spacing keeps sub-millimeter steps sharp).
pub fn is_stepped(gradient: &StyleProperty<Color>) -> bool {
    let Some(crate::style::expression::Expression::Step { stops, .. }) = gradient.expression()
    else {
        return false;
    };
    stops.iter().any(|(stop, _)| *stop > 0.0 && *stop < 0.01)
}

fn needs_high_res_linear(gradient: &StyleProperty<Color>) -> bool {
    gradient.expression().is_some_and(expr_needs_high_res)
}

fn expr_needs_high_res(expr: &crate::style::expression::Expression) -> bool {
    use crate::style::expression::Expression;
    match expr {
        Expression::Step { .. } => true,
        Expression::Interpolate { stops, .. } => {
            stops
                .windows(2)
                .any(|pair| (pair[1].0 - pair[0].0).abs() < 0.02)
                || stops.iter().any(|(_, out)| expr_needs_high_res(out))
        }
        Expression::Case {
            branches, fallback, ..
        } => {
            branches
                .iter()
                .any(|(cond, out)| expr_needs_high_res(cond) || expr_needs_high_res(out))
                || expr_needs_high_res(fallback)
        }
        Expression::Match {
            input,
            cases,
            fallback,
            ..
        } => {
            expr_needs_high_res(input)
                || cases.iter().any(|(_, out)| expr_needs_high_res(out))
                || expr_needs_high_res(fallback)
        }
        Expression::Let { bindings, body } => {
            bindings.iter().any(|(_, val)| expr_needs_high_res(val)) || expr_needs_high_res(body)
        }
        Expression::Coalesce { operands, .. } => operands.iter().any(expr_needs_high_res),
        _ => false,
    }
}

/// The colour of each of [`RAMP_TEXELS`], [`HIGH_RES_RAMP_TEXELS`], or [`STEP_RAMP_TEXELS`]
/// positions along the line, premultiplied 8-bit RGBA so linear GPU filtering does not fringe
/// transparent stops. A position whose colour cannot be evaluated is transparent.
pub fn ramp(gradient: &StyleProperty<Color>) -> Vec<[u8; 4]> {
    let log_stepped = is_stepped(gradient);
    let texels = if log_stepped {
        STEP_RAMP_TEXELS
    } else if needs_high_res_linear(gradient) {
        HIGH_RES_RAMP_TEXELS
    } else {
        RAMP_TEXELS
    };
    (0..texels)
        .map(|texel| {
            let position = texel as f64 / (texels - 1) as f64;
            let progress = if log_stepped {
                stepped_progress(position)
            } else {
                position
            };
            let color = match gradient {
                StyleProperty::Constant(color) => Some(ExpressionColor::from(*color)),
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
            let [r, g, b, a] = color.map_or([0.0; 4], |color| color.premultiplied());
            [r, g, b, a].map(|channel| (channel * 255.0).round() as u8)
        })
        .collect()
}

#[cfg(test)]
mod tests;
