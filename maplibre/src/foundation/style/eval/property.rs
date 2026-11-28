//! Style property values: a constant, or an expression evaluated per feature and per zoom.

use std::sync::Arc;

pub mod compiled;
mod resampling;
mod spec_bridge;
mod text;
mod transition;
pub use compiled::{CompiledProperty, ZoomCurve};
pub use resampling::{RasterResampling, ResamplingMode};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
pub use spec_bridge::lookup_legacy_property_spec;
pub use text::{FormattedText, ImageName, PatternName, TextField, TextSection};
#[cfg(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl"))]
pub(crate) use transition::extract_layer_transitions_ref;
pub use transition::{ActiveTransition, ActiveTransitions, TransitionSpec};

use crate::style::expression::{
    EvaluationContext, Expression, FeatureProperties, LegacyPropertySpec, ParseError, PropertyKind,
    Value,
};

/// A type a style property can hold, with the specification the expression engine needs to
/// lower legacy syntax into it.
pub trait PropertyValue: Clone + Sized {
    /// What the specification says about properties of this type.
    fn spec() -> LegacyPropertySpec;

    /// The value an expression produced, if it fits this type.
    fn from_value(value: &Value) -> Option<Self>;

    /// A constant written directly in the style that the expression engine would not accept
    /// as this type, such as an array of colour strings; `None` hands the JSON to the engine.
    fn from_literal(_json: &serde_json::Value) -> Option<Self> {
        None
    }
}

/// One number or several, what a `numberArray` property such as a light direction holds.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NumberList(pub Vec<f64>);

impl PropertyValue for NumberList {
    fn spec() -> LegacyPropertySpec {
        LegacyPropertySpec::interpolated(PropertyKind::NumberArray)
    }

    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::NumberArray(numbers) => numbers
                .0
                .iter()
                .all(|n| n.is_finite())
                .then(|| Self(numbers.0.clone())),
            Value::Number(number) => number.is_finite().then(|| Self(vec![*number])),
            Value::Array(items) => items
                .iter()
                .map(|item| item.as_number().filter(|n| n.is_finite()))
                .collect::<Option<Vec<f64>>>()
                .map(Self),
            _ => None,
        }
    }

    fn from_literal(json: &serde_json::Value) -> Option<Self> {
        json.as_array()?
            .iter()
            .map(serde_json::Value::as_f64)
            .collect::<Option<Vec<f64>>>()
            .map(Self)
    }
}

/// One colour or several, what a `colorArray` property such as a shadow colour holds.
#[derive(Clone, Debug, PartialEq)]
pub struct ColorList(pub Vec<crate::style::expression::Color>);

impl Serialize for ColorList {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.0.iter().map(crate::style::expression::Color::css))
    }
}

impl PropertyValue for ColorList {
    fn spec() -> LegacyPropertySpec {
        LegacyPropertySpec::interpolated(PropertyKind::ColorArray)
    }

    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::ColorArray(colors) => Some(Self(colors.0.clone())),
            Value::Color(color) => Some(Self(vec![*color])),
            Value::String(text) => {
                crate::style::expression::Color::parse(text).map(|color| Self(vec![color]))
            }
            Value::Array(items) => items
                .iter()
                .map(|item| match item {
                    Value::Color(color) => Some(*color),
                    Value::String(text) => crate::style::expression::Color::parse(text),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()
                .map(Self),
            _ => None,
        }
    }

    fn from_literal(json: &serde_json::Value) -> Option<Self> {
        json.as_array()?
            .iter()
            .map(|item| crate::style::expression::Color::parse(item.as_str()?))
            .collect::<Option<Vec<_>>>()
            .map(Self)
    }
}

impl PropertyValue for f32 {
    fn spec() -> LegacyPropertySpec {
        LegacyPropertySpec::interpolated(PropertyKind::Number)
    }

    fn from_value(value: &Value) -> Option<Self> {
        value
            .as_number()
            .filter(|number| number.is_finite())
            .map(|number| number as f32)
            .filter(|number| number.is_finite())
    }
}

impl PropertyValue for f64 {
    fn spec() -> LegacyPropertySpec {
        LegacyPropertySpec::interpolated(PropertyKind::Number)
    }

    fn from_value(value: &Value) -> Option<Self> {
        value.as_number().filter(|number| number.is_finite())
    }
}

impl PropertyValue for bool {
    fn spec() -> LegacyPropertySpec {
        LegacyPropertySpec::stepped(PropertyKind::Boolean)
    }

    fn from_value(value: &Value) -> Option<Self> {
        value.as_bool()
    }
}

impl PropertyValue for String {
    fn spec() -> LegacyPropertySpec {
        LegacyPropertySpec::stepped(PropertyKind::String)
    }

    fn from_value(value: &Value) -> Option<Self> {
        value.as_str().map(str::to_string)
    }
}

impl PropertyValue for csscolorparser::Color {
    fn spec() -> LegacyPropertySpec {
        LegacyPropertySpec::interpolated(PropertyKind::Color)
    }

    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Color(color) => Some((*color).into()),
            _ => None,
        }
    }
}

impl PropertyValue for [f64; 3] {
    fn spec() -> LegacyPropertySpec {
        LegacyPropertySpec::interpolated(PropertyKind::Array {
            item: Box::new(PropertyKind::Number),
            length: Some(3),
        })
    }

    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Array(items) => match items.as_slice() {
                [x, y, z] => Some([
                    x.as_number().filter(|n| n.is_finite())?,
                    y.as_number().filter(|n| n.is_finite())?,
                    z.as_number().filter(|n| n.is_finite())?,
                ]),
                _ => None,
            },
            _ => None,
        }
    }
}

/// A parsed expression together with the JSON it came from.
#[derive(Debug, Clone)]
pub struct PropertyExpression {
    source: serde_json::Value,
    expression: Expression,
}

impl PropertyExpression {
    /// The parsed expression.
    pub fn expression(&self) -> &Expression {
        &self.expression
    }

    /// The JSON the expression was parsed from.
    pub fn source(&self) -> &serde_json::Value {
        &self.source
    }

    #[cfg(test)]
    pub(crate) fn from_worker_expression(expression: Expression) -> Self {
        Self {
            source: serde_json::Value::Null,
            expression,
        }
    }
}

/// A property value the engine could not parse.
#[derive(Debug, Clone)]
pub struct UnsupportedProperty {
    /// The JSON as written in the style.
    pub source: serde_json::Value,
    /// Why it was rejected.
    pub error: ParseError,
}

/// The value of a style property.
///
/// Parsed values sit behind an `Arc`, so a property stays as small as its constant and a
/// paint struct of many properties stays compact.
#[derive(Debug, Clone)]
pub enum StyleProperty<T> {
    /// The same value for every feature at every zoom.
    Constant(T),
    /// A value computed per feature or per zoom.
    Expression(Arc<PropertyExpression>),
    /// A value the engine could not parse; it evaluates to nothing so the default applies.
    Unsupported(Arc<UnsupportedProperty>),
}

#[inline(never)]
fn eval_expr_at(
    property: &PropertyExpression,
    context: &EvaluationContext<'_>,
) -> Option<crate::style::expression::Value> {
    property.expression.evaluate(context).ok()
}

impl<T: PropertyValue> StyleProperty<T> {
    /// Parses a property value: a constant, a legacy function object or an expression.
    pub fn parse(json: &serde_json::Value) -> Self {
        Self::parse_with_spec(json, &T::spec())
    }

    /// Evaluates the property; `None` when the expression fails or was unsupported, so the
    /// caller applies the specification default.
    pub fn evaluate(&self, context: &EvaluationContext) -> Option<T> {
        match self {
            Self::Constant(value) => Some(value.clone()),
            Self::Expression(property) => {
                let value = eval_expr_at(property, context)?;
                T::from_value(&value)
            }
            Self::Unsupported(_) => None,
        }
    }

    /// Evaluates the property for a feature at a zoom.
    pub fn evaluate_for(&self, properties: &FeatureProperties, zoom: f64) -> Option<T> {
        self.evaluate(&EvaluationContext::for_feature(zoom, properties))
    }

    /// Evaluates the property at a zoom, without a feature.
    pub fn evaluate_at_zoom(&self, zoom: f64) -> Option<T> {
        self.evaluate(&EvaluationContext::at_zoom(zoom))
    }

    /// Whether the value is the same for every feature.
    pub fn is_feature_constant(&self) -> bool {
        self.expression()
            .is_none_or(Expression::is_feature_constant)
    }

    /// Whether the value is the same at every zoom.
    pub fn is_zoom_constant(&self) -> bool {
        self.expression().is_none_or(Expression::is_zoom_constant)
    }

    /// The parsed expression, when the property is one.
    pub fn expression(&self) -> Option<&Expression> {
        match self {
            Self::Expression(property) => Some(&property.expression),
            _ => None,
        }
    }

    /// Deserializes an optional property; kept as a named function for `deserialize_with`.
    pub fn deserialize_or_none<'de, D>(deserializer: D) -> Result<Option<Self>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<Self>::deserialize(deserializer)
    }

    /// Deserializes an optional colour property.
    pub fn deserialize_color_or_none<'de, D>(deserializer: D) -> Result<Option<Self>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::deserialize_or_none(deserializer)
    }
}

impl StyleProperty<f32> {
    /// Deserializes an optional number property.
    pub fn deserialize_f32_or_none<'de, D>(deserializer: D) -> Result<Option<Self>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::deserialize_or_none(deserializer)
    }

    fn two_stop_zoom_interpolation(
        &self,
    ) -> Option<(&crate::style::expression::Interpolation, f64, f64)> {
        let Expression::Interpolate {
            interpolation,
            input,
            stops,
            ..
        } = self.expression()?
        else {
            return None;
        };
        if !matches!(
            **input,
            Expression::Global(crate::style::expression::Global::Zoom)
        ) {
            return None;
        }
        let [(z0, _), (z1, _)] = stops.as_slice() else {
            return None;
        };
        Some((interpolation, *z0, *z1))
    }

    /// Lower and upper zoom stops when the property is a two-stop top-level zoom `interpolate`.
    pub fn zoom_interpolation_bounds(&self) -> Option<(f64, f64)> {
        let (_, z0, z1) = self.two_stop_zoom_interpolation()?;
        Some((z0, z1))
    }

    /// Continuous `[0.0, 1.0]` interpolation factor at `zoom` for a two-stop top-level zoom
    /// `interpolate` expression.
    pub fn zoom_interpolation_factor(&self, zoom: f64) -> Option<f32> {
        let (interpolation, z0, z1) = self.two_stop_zoom_interpolation()?;
        let factor = if zoom <= z0 {
            0.0
        } else if zoom >= z1 {
            1.0
        } else {
            interpolation.factor(zoom, z0, z1).clamp(0.0, 1.0)
        };
        Some(factor as f32)
    }
}

impl<T: PropertyValue + Serialize> Serialize for StyleProperty<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Constant(value) => value.serialize(serializer),
            Self::Expression(property) => property.source.serialize(serializer),
            Self::Unsupported(unsupported) => unsupported.source.serialize(serializer),
        }
    }
}

impl<'de, T: PropertyValue> Deserialize<'de> for StyleProperty<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let json = serde_json::Value::deserialize(deserializer)?;
        Ok(Self::parse(&json))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{NumberList, StyleProperty};
    use crate::style::expression::{FeatureProperties, Value};

    #[test]
    fn non_finite_constant_folded_expressions_evaluate_to_none_and_stay_valid() {
        let nan_f32 = StyleProperty::<f32>::parse(&json!(["sqrt", -1]));
        assert!(
            matches!(nan_f32, StyleProperty::Expression(_)),
            "folded NaN must remain a valid Expression, not Constant(NaN) or Unsupported"
        );
        assert_eq!(nan_f32.evaluate_at_zoom(5.0), None);

        let inf_f64 = StyleProperty::<f64>::parse(&json!(["/", 1, 0]));
        assert!(matches!(inf_f64, StyleProperty::Expression(_)));
        assert_eq!(inf_f64.evaluate_at_zoom(5.0), None);

        let nan_vec3 = StyleProperty::<[f64; 3]>::parse(&json!(["literal", [1.0, 2.0, 3.0]]));
        assert_eq!(nan_vec3.evaluate_at_zoom(5.0), Some([1.0, 2.0, 3.0]));
        let nan_vec3_bad =
            StyleProperty::<[f64; 3]>::parse(&json!(["array", "number", 3, ["get", "v"]]));
        let mut props = FeatureProperties::default();
        props.insert(
            "v".to_string(),
            Value::Array(vec![
                Value::Number(1.0),
                Value::Number(f64::NAN),
                Value::Number(3.0),
            ]),
        );
        assert_eq!(nan_vec3_bad.evaluate_for(&props, 5.0), None);

        let nan_list = StyleProperty::<NumberList>::parse(&json!(["sqrt", -1]));
        assert_eq!(nan_list.evaluate_at_zoom(5.0), None);
    }

    #[test]
    fn data_driven_nan_falls_back_while_finite_values_succeed() {
        let prop = StyleProperty::<f32>::parse(&json!(["sqrt", ["get", "x"]]));
        let mut neg = FeatureProperties::default();
        neg.insert("x".to_string(), Value::Number(-1.0));
        assert_eq!(prop.evaluate_for(&neg, 5.0), None);

        let mut pos = FeatureProperties::default();
        pos.insert("x".to_string(), Value::Number(9.0));
        assert_eq!(prop.evaluate_for(&pos, 5.0), Some(3.0));
    }
}
