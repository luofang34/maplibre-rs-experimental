//! Fast compiled 1D linear ramp evaluators for zoom-constant per-feature expressions.

use crate::style::{
    expression::{Color, ColorSpace, Expression, FeatureProperties, Interpolation, Value},
    property::StyleProperty,
};

struct PropertyInput<'a> {
    key: &'a str,
    fallback: Option<f64>,
}

/// Zero-allocation 1D linear scalar interpolation ramp (`["interpolate", ["linear"], input, ...]`).
pub(super) struct ScalarRamp<'a> {
    input: PropertyInput<'a>,
    stops: &'a [(f64, Expression)],
}

/// Zero-allocation 1D linear RGBA interpolation ramp (`["interpolate", ["linear"], input, ...]`).
pub(super) struct ColorRamp<'a> {
    input: PropertyInput<'a>,
    stops: &'a [(f64, Expression)],
}

impl<'a> ScalarRamp<'a> {
    /// Returns `Some` when `prop` is a simple 1D linear scalar ramp over one feature property.
    pub(super) fn compile(prop: &'a StyleProperty<f32>) -> Option<Self> {
        let StyleProperty::Expression(expr_prop) = prop else {
            return None;
        };
        let Expression::Interpolate {
            interpolation: Interpolation::Linear,
            input,
            stops,
            ..
        } = expr_prop.expression()
        else {
            return None;
        };
        if stops.is_empty() || !stops.iter().all(|(_, out)| stop_number(out).is_some()) {
            return None;
        }
        Some(Self {
            input: compile_input(input)?,
            stops,
        })
    }

    #[inline]
    pub(super) fn eval(&self, props: &FeatureProperties) -> Option<f32> {
        let x = eval_input(props, &self.input)?;
        if self.stops.len() == 1 || x <= self.stops[0].0 {
            return stop_number(&self.stops[0].1).map(|v| v as f32);
        }
        let last = self.stops.len() - 1;
        if x >= self.stops[last].0 {
            return stop_number(&self.stops[last].1).map(|v| v as f32);
        }
        for pair in self.stops.windows(2) {
            let (x0, e0) = &pair[0];
            let (x1, e1) = &pair[1];
            if x <= *x1 {
                let y0 = stop_number(e0)?;
                let y1 = stop_number(e1)?;
                let span = *x1 - *x0;
                let t = if span > 0.0 {
                    ((x - *x0) / span).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                return Some((y0 + (y1 - y0) * t) as f32);
            }
        }
        stop_number(&self.stops[last].1).map(|v| v as f32)
    }
}

impl<'a> ColorRamp<'a> {
    /// Returns `Some` when `prop` is a simple 1D linear sRGB color ramp over one feature property.
    pub(super) fn compile<T>(prop: &'a StyleProperty<T>) -> Option<Self> {
        let StyleProperty::Expression(expr_prop) = prop else {
            return None;
        };
        let Expression::Interpolate {
            interpolation: Interpolation::Linear,
            space: ColorSpace::Rgb,
            input,
            stops,
            ..
        } = expr_prop.expression()
        else {
            return None;
        };
        if stops.is_empty() || !stops.iter().all(|(_, out)| stop_color(out).is_some()) {
            return None;
        }
        Some(Self {
            input: compile_input(input)?,
            stops,
        })
    }

    #[inline]
    pub(super) fn eval(&self, props: &FeatureProperties) -> Option<[f32; 4]> {
        let x = eval_input(props, &self.input)?;
        let to_f32 = |c: Color| {
            let [r, g, b, a] = c.straight();
            [r as f32, g as f32, b as f32, a as f32]
        };
        if self.stops.len() == 1 || x <= self.stops[0].0 {
            return stop_color(&self.stops[0].1).map(to_f32);
        }
        let last = self.stops.len() - 1;
        if x >= self.stops[last].0 {
            return stop_color(&self.stops[last].1).map(to_f32);
        }
        for pair in self.stops.windows(2) {
            let (x0, e0) = &pair[0];
            let (x1, e1) = &pair[1];
            if x <= *x1 {
                let c0 = stop_color(e0)?.straight();
                let c1 = stop_color(e1)?.straight();
                let span = *x1 - *x0;
                let t = if span > 0.0 {
                    ((x - *x0) / span).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                return Some([
                    (c0[0] + (c1[0] - c0[0]) * t) as f32,
                    (c0[1] + (c1[1] - c0[1]) * t) as f32,
                    (c0[2] + (c1[2] - c0[2]) * t) as f32,
                    (c0[3] + (c1[3] - c0[3]) * t) as f32,
                ]);
            }
        }
        stop_color(&self.stops[last].1).map(to_f32)
    }
}

fn compile_input(expr: &Expression) -> Option<PropertyInput<'_>> {
    match expr {
        Expression::Get { key, object: None } => Some(PropertyInput {
            key: literal_str(key)?,
            fallback: None,
        }),
        Expression::Coalesce { operands, .. } if operands.len() == 2 => {
            let Expression::Get { key, object: None } = &operands[0] else {
                return None;
            };
            Some(PropertyInput {
                key: literal_str(key)?,
                fallback: Some(stop_number(&operands[1])?),
            })
        }
        _ => None,
    }
}

#[inline]
fn eval_input(props: &FeatureProperties, input: &PropertyInput<'_>) -> Option<f64> {
    props
        .get(input.key)
        .and_then(Value::as_number)
        .or(input.fallback)
}

#[inline]
fn literal_str(expr: &Expression) -> Option<&str> {
    match expr {
        Expression::Literal(Value::String(s))
        | Expression::Folded {
            value: Value::String(s),
            ..
        } => Some(s.as_str()),
        _ => None,
    }
}

#[inline]
fn stop_number(expr: &Expression) -> Option<f64> {
    match expr {
        Expression::Literal(Value::Number(v))
        | Expression::Folded {
            value: Value::Number(v),
            ..
        } => Some(*v),
        _ => None,
    }
}

#[inline]
fn stop_color(expr: &Expression) -> Option<Color> {
    match expr {
        Expression::Literal(Value::Color(c))
        | Expression::Folded {
            value: Value::Color(c),
            ..
        } => Some(*c),
        Expression::Literal(Value::String(s))
        | Expression::Folded {
            value: Value::String(s),
            ..
        } => Color::parse(s),
        _ => None,
    }
}
