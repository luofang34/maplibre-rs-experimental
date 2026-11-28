//! Fast compiled 1D linear ramp evaluators for zoom-constant per-feature expressions.

use crate::style::{
    expression::{Color, ColorSpace, Expression, FeatureProperties, Interpolation, Type, Value},
    property::StyleProperty,
};

struct PropertyInput {
    key: String,
    fallback: Option<f64>,
}

impl PropertyInput {
    #[inline]
    fn read_props(&self, props: &FeatureProperties) -> Option<f64> {
        match props.get(self.key.as_str()) {
            Some(Value::Number(v)) if !v.is_nan() => Some(*v),
            None | Some(Value::Null) => self.fallback,
            _ => None,
        }
    }
}

enum Stops<T: Copy> {
    Two {
        x0: f64,
        v0: T,
        x1: f64,
        v1: T,
        inv01: f64,
    },
    Three {
        x0: f64,
        v0: T,
        x1: f64,
        v1: T,
        inv01: f64,
        x2: f64,
        v2: T,
        inv12: f64,
    },
    General(Vec<(f64, T, f64)>),
}

#[inline]
fn inv_span(x0: f64, x1: f64) -> f64 {
    let span = x1 - x0;
    if span > 0.0 {
        1.0 / span
    } else {
        0.0
    }
}

impl<T: Copy> Stops<T> {
    fn from_exprs<F>(stops: &[(f64, Expression)], mut convert: F) -> Option<Self>
    where
        F: FnMut(&Expression) -> Option<T>,
    {
        let items: Vec<(f64, T)> = stops
            .iter()
            .map(|(x, expr)| Some((*x, convert(expr)?)))
            .collect::<Option<Vec<_>>>()?;
        match items.as_slice() {
            [] | [_] => None,
            [(x0, v0), (x1, v1)] => Some(Self::Two {
                x0: *x0,
                v0: *v0,
                x1: *x1,
                v1: *v1,
                inv01: inv_span(*x0, *x1),
            }),
            [(x0, v0), (x1, v1), (x2, v2)] => Some(Self::Three {
                x0: *x0,
                v0: *v0,
                x1: *x1,
                v1: *v1,
                inv01: inv_span(*x0, *x1),
                x2: *x2,
                v2: *v2,
                inv12: inv_span(*x1, *x2),
            }),
            _ => {
                let mut general = Vec::with_capacity(items.len());
                for (i, &(x, v)) in items.iter().enumerate() {
                    let inv = items
                        .get(i + 1)
                        .map_or(0.0, |(next_x, _)| inv_span(x, *next_x));
                    general.push((x, v, inv));
                }
                Some(Self::General(general))
            }
        }
    }

    #[inline(always)]
    fn eval<O: Default, E, L>(&self, x: f64, endpoint: E, lerp: L) -> O
    where
        E: FnOnce(T) -> O,
        L: FnOnce(T, T, f64) -> O,
    {
        match *self {
            Self::Two {
                x0,
                v0,
                x1,
                v1,
                inv01,
            } => {
                if x <= x0 {
                    endpoint(v0)
                } else if x >= x1 {
                    endpoint(v1)
                } else {
                    lerp(v0, v1, lerp_t(x, x0, inv01))
                }
            }
            Self::Three {
                x0,
                v0,
                x1,
                v1,
                inv01,
                x2,
                v2,
                inv12,
            } => {
                if x <= x0 {
                    endpoint(v0)
                } else if x <= x1 {
                    lerp(v0, v1, lerp_t(x, x0, inv01))
                } else if x >= x2 {
                    endpoint(v2)
                } else {
                    lerp(v1, v2, lerp_t(x, x1, inv12))
                }
            }
            Self::General(ref stops) => eval_general(stops, x, endpoint, lerp),
        }
    }
}

#[inline(always)]
fn lerp_t(x: f64, x0: f64, inv: f64) -> f64 {
    if inv > 0.0 {
        ((x - x0) * inv).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn eval_general<T: Copy, O: Default, E, L>(
    stops: &[(f64, T, f64)],
    x: f64,
    endpoint: E,
    lerp: L,
) -> O
where
    E: FnOnce(T) -> O,
    L: FnOnce(T, T, f64) -> O,
{
    let Some(&(first_x, first_v, _)) = stops.first() else {
        return O::default();
    };
    if x <= first_x {
        return endpoint(first_v);
    }
    let Some(&(last_x, last_v, _)) = stops.last() else {
        return endpoint(first_v);
    };
    if x >= last_x {
        return endpoint(last_v);
    }
    for pair in stops.windows(2) {
        let [(x0, v0, inv01), (x1, v1, _)] = *pair else {
            continue;
        };
        if x <= x1 {
            return lerp(v0, v1, lerp_t(x, x0, inv01));
        }
    }
    endpoint(last_v)
}

/// Zero-allocation 1D linear scalar interpolation ramp (`["interpolate", ["linear"], input, ...]`).
pub(super) struct ScalarRamp {
    input: PropertyInput,
    stops: Stops<f64>,
}

/// Zero-allocation 1D linear RGBA interpolation ramp (`["interpolate", ["linear"], input, ...]`).
pub(super) struct ColorRamp {
    input: PropertyInput,
    stops: Stops<[f32; 4]>,
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
            stops: Stops::from_exprs(stops, stop_number)?,
        })
    }

    #[inline]
    pub(super) fn key(&self) -> &str {
        &self.input.key
    }

    #[inline]
    pub(super) fn eval(&self, props: &FeatureProperties) -> Option<f32> {
        Some(self.eval_x(self.input.read_props(props)?))
    }

    #[inline]
    pub(super) fn eval_f64(&self, raw: Option<f64>) -> Option<f32> {
        let x = raw.or(self.input.fallback)?;
        if x.is_nan() {
            return None;
        }
        Some(self.eval_x(x))
    }

    #[inline]
    pub(super) fn eval_x(&self, x: f64) -> f32 {
        self.stops
            .eval(x, |v| v as f32, |y0, y1, t| (y0 + (y1 - y0) * t) as f32)
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
            stops: Stops::from_exprs(stops, to_f32)?,
        })
    }

    #[inline]
    pub(super) fn key(&self) -> &str {
        &self.input.key
    }

    #[inline]
    pub(super) fn eval(&self, props: &FeatureProperties) -> Option<[f32; 4]> {
        Some(self.eval_x(self.input.read_props(props)?))
    }

    #[inline]
    pub(super) fn eval_f64(&self, raw: Option<f64>) -> Option<[f32; 4]> {
        let x = raw.or(self.input.fallback)?;
        if x.is_nan() {
            return None;
        }
        Some(self.eval_x(x))
    }

    #[inline]
    pub(super) fn eval_x(&self, x: f64) -> [f32; 4] {
        self.stops.eval(
            x,
            |c| c,
            |c0, c1, t| {
                let t = t as f32;
                [
                    c0[0] + (c1[0] - c0[0]) * t,
                    c0[1] + (c1[1] - c0[1]) * t,
                    c0[2] + (c1[2] - c0[2]) * t,
                    c0[3] + (c1[3] - c0[3]) * t,
                ]
            },
        )
    }
}

fn compile_input(expr: &Expression) -> Option<PropertyInput> {
    let inner = match expr {
        Expression::Assert {
            required: Type::Number,
            operands,
        } if operands.len() == 1 => operands.first()?,
        other => other,
    };
    match inner {
        Expression::Get { key, object: None } => Some(PropertyInput {
            key: literal_str(key)?.to_owned(),
            fallback: None,
        }),
        Expression::Coalesce { operands, .. } => {
            let [Expression::Get { key, object: None }, fallback] = operands.as_slice() else {
                return None;
            };
            let fallback = stop_number(fallback)?;
            if fallback.is_nan() {
                return None;
            }
            Some(PropertyInput {
                key: literal_str(key)?.to_owned(),
                fallback: Some(fallback),
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

#[cfg(test)]
mod tests {
    use super::{ColorRamp, ScalarRamp};
    use crate::style::{
        expression::{FeatureProperties, Value},
        property::{PropertyValue, StyleProperty},
    };

    fn parse_prop<T: PropertyValue>(raw: &str) -> StyleProperty<T> {
        let json: serde_json::Value = serde_json::from_str(raw).expect("valid test json");
        StyleProperty::<T>::parse(&json)
    }

    #[test]
    fn specialized_ramps_match_linear_interpolation() {
        let two = parse_prop::<f32>(
            r#"["interpolate",["linear"],["coalesce",["get","v"],0.5],0,2.0,10,12.0]"#,
        );
        let r2 = ScalarRamp::compile(&two).expect("compile 2");
        assert_eq!(r2.eval_f64(Some(-1.0)), Some(2.0));
        assert_eq!(r2.eval_f64(Some(5.0)), Some(7.0));
        assert_eq!(r2.eval_f64(Some(12.0)), Some(12.0));
        assert_eq!(r2.eval_f64(None), Some(2.5));

        let three =
            parse_prop::<f32>(r#"["interpolate",["linear"],["get","v"],0,0.0,10,10.0,20,30.0]"#);
        let r3 = ScalarRamp::compile(&three).expect("compile 3");
        assert_eq!(r3.eval_f64(Some(5.0)), Some(5.0));
        assert_eq!(r3.eval_f64(Some(15.0)), Some(20.0));
        assert_eq!(r3.eval_f64(None), None);

        let four = parse_prop::<f32>(
            r#"["interpolate",["linear"],["get","v"],0,0.0,10,10.0,20,30.0,30,50.0]"#,
        );
        let r4 = ScalarRamp::compile(&four).expect("compile 4");
        assert_eq!(r4.eval_f64(Some(25.0)), Some(40.0));

        let c2 = parse_prop::<csscolorparser::Color>(
            r##"["interpolate",["linear"],["coalesce",["get","v"],0.0],0,"#000000",10,"#ffffff"]"##,
        );
        let rc2 = ColorRamp::compile(&c2).expect("compile color 2");
        let mid = rc2.eval_f64(Some(5.0)).expect("mid");
        assert!((mid[0] - 0.5).abs() < 1e-6 && (mid[3] - 1.0).abs() < 1e-6);

        let c4 = parse_prop::<csscolorparser::Color>(
            r##"["interpolate",["linear"],["get","v"],0,"#000000",10,"#800000",20,"#008000",30,"#0000ff"]"##,
        );
        assert!(ColorRamp::compile(&c4).is_some());
    }

    #[test]
    fn ramps_match_style_property_semantics_on_missing_null_and_non_number() {
        let prop = parse_prop::<f32>(
            r#"["interpolate",["linear"],["coalesce",["get","v"],0.5],0,2.0,10,12.0]"#,
        );
        let ramp = ScalarRamp::compile(&prop).expect("compile coalesce");
        let color_prop = parse_prop::<csscolorparser::Color>(
            r##"["interpolate",["linear"],["coalesce",["get","v"],0.5],0,"#000000",10,"#ffffff"]"##,
        );
        let color_ramp = ColorRamp::compile(&color_prop).expect("compile color coalesce");

        let cases = [
            FeatureProperties::default(),
            FeatureProperties::from_iter([("v".to_owned(), Value::Number(5.0))]),
            FeatureProperties::from_iter([("v".to_owned(), Value::Null)]),
            FeatureProperties::from_iter([("v".to_owned(), Value::String("bad".to_owned()))]),
        ];
        for props in &cases {
            assert_eq!(ramp.eval(props), prop.evaluate_for(props, 0.0));
            assert_eq!(
                color_ramp.eval(props),
                color_prop.evaluate_for(props, 0.0).map(|c| c.to_array())
            );
        }
    }
}
