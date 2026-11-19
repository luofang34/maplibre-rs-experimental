//! Fast compiled 1D linear ramp evaluators for zoom-constant per-feature expressions.

use crate::style::{
    expression::{Color, ColorSpace, Expression, FeatureProperties, Interpolation, Value},
    property::StyleProperty,
};

const MAX_INLINE_STOPS: usize = 16;

struct PropertyInput {
    key: String,
    fallback: Option<f64>,
}

enum RampStops<T: Copy> {
    Inline {
        len: u8,
        items: [(f64, T); MAX_INLINE_STOPS],
    },
    Heap(Vec<(f64, T)>),
}

impl<T: Copy> RampStops<T> {
    fn from_exprs<F>(stops: &[(f64, Expression)], default: T, mut convert: F) -> Option<Self>
    where
        F: FnMut(&Expression) -> Option<T>,
    {
        if stops.is_empty() {
            return None;
        }
        if stops.len() <= MAX_INLINE_STOPS {
            let mut items = [(0.0, default); MAX_INLINE_STOPS];
            for (slot, (x, expr)) in items.iter_mut().zip(stops.iter()) {
                *slot = (*x, convert(expr)?);
            }
            return Some(Self::Inline {
                len: stops.len() as u8,
                items,
            });
        }
        let mut heap = Vec::with_capacity(stops.len());
        for (x, expr) in stops {
            heap.push((*x, convert(expr)?));
        }
        Some(Self::Heap(heap))
    }

    #[inline]
    fn as_slice(&self) -> &[(f64, T)] {
        match self {
            Self::Inline { len, items } => &items[..usize::from(*len)],
            Self::Heap(items) => items.as_slice(),
        }
    }
}

/// Zero-allocation 1D linear scalar interpolation ramp (`["interpolate", ["linear"], input, ...]`).
pub(super) struct ScalarRamp {
    input: PropertyInput,
    stops: RampStops<f64>,
}

/// Zero-allocation 1D linear RGBA interpolation ramp (`["interpolate", ["linear"], input, ...]`).
pub(super) struct ColorRamp {
    input: PropertyInput,
    stops: RampStops<[f32; 4]>,
}

impl ScalarRamp {
    /// Returns `Some` when `prop` is a simple 1D linear scalar ramp over one feature property.
    pub(super) fn compile(prop: &StyleProperty<f32>) -> Option<Self> {
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
        Some(Self {
            input: compile_input(input)?,
            stops: RampStops::from_exprs(stops, 0.0, stop_number)?,
        })
    }

    #[inline]
    pub(super) fn key(&self) -> &str {
        &self.input.key
    }

    #[inline]
    pub(super) fn eval(&self, props: &FeatureProperties) -> Option<f32> {
        let raw = props
            .get(self.input.key.as_str())
            .and_then(Value::as_number);
        self.eval_f64(raw)
    }

    #[inline]
    pub(super) fn eval_f64(&self, raw: Option<f64>) -> Option<f32> {
        let x = raw.or(self.input.fallback)?;
        let stops = self.stops.as_slice();
        if stops.len() == 1 || x <= stops[0].0 {
            return Some(stops[0].1 as f32);
        }
        let last = stops.len() - 1;
        if x >= stops[last].0 {
            return Some(stops[last].1 as f32);
        }
        for pair in stops.windows(2) {
            let (x0, y0) = pair[0];
            let (x1, y1) = pair[1];
            if x <= x1 {
                let span = x1 - x0;
                let t = if span > 0.0 {
                    ((x - x0) / span).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                return Some((y0 + (y1 - y0) * t) as f32);
            }
        }
        Some(stops[last].1 as f32)
    }
}

impl ColorRamp {
    /// Returns `Some` when `prop` is a simple 1D linear sRGB color ramp over one feature property.
    pub(super) fn compile<T>(prop: &StyleProperty<T>) -> Option<Self> {
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
        let to_f32 = |expr: &Expression| {
            let [r, g, b, a] = stop_color(expr)?.straight();
            Some([r as f32, g as f32, b as f32, a as f32])
        };
        Some(Self {
            input: compile_input(input)?,
            stops: RampStops::from_exprs(stops, [0.0; 4], to_f32)?,
        })
    }

    #[inline]
    pub(super) fn key(&self) -> &str {
        &self.input.key
    }

    #[inline]
    pub(super) fn eval(&self, props: &FeatureProperties) -> Option<[f32; 4]> {
        let raw = props
            .get(self.input.key.as_str())
            .and_then(Value::as_number);
        self.eval_f64(raw)
    }

    #[inline]
    pub(super) fn eval_f64(&self, raw: Option<f64>) -> Option<[f32; 4]> {
        let x = raw.or(self.input.fallback)?;
        let stops = self.stops.as_slice();
        if stops.len() == 1 || x <= stops[0].0 {
            return Some(stops[0].1);
        }
        let last = stops.len() - 1;
        if x >= stops[last].0 {
            return Some(stops[last].1);
        }
        for pair in stops.windows(2) {
            let (x0, c0) = pair[0];
            let (x1, c1) = pair[1];
            if x <= x1 {
                let span = x1 - x0;
                let t = if span > 0.0 {
                    ((x - x0) / span).clamp(0.0, 1.0)
                } else {
                    0.0
                } as f32;
                return Some([
                    c0[0] + (c1[0] - c0[0]) * t,
                    c0[1] + (c1[1] - c0[1]) * t,
                    c0[2] + (c1[2] - c0[2]) * t,
                    c0[3] + (c1[3] - c0[3]) * t,
                ]);
            }
        }
        Some(stops[last].1)
    }
}

fn compile_input(expr: &Expression) -> Option<PropertyInput> {
    match expr {
        Expression::Get { key, object: None } => Some(PropertyInput {
            key: literal_str(key)?.to_owned(),
            fallback: None,
        }),
        Expression::Coalesce { operands, .. } if operands.len() == 2 => {
            let Expression::Get { key, object: None } = &operands[0] else {
                return None;
            };
            Some(PropertyInput {
                key: literal_str(key)?.to_owned(),
                fallback: Some(stop_number(&operands[1])?),
            })
        }
        _ => None,
    }
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
