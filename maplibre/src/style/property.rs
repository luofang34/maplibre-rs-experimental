//! Style property values: a constant, or an expression evaluated per feature and per zoom.

use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::style::expression::{
    EvaluationContext, Expression, FeatureProperties, LegacyPropertySpec, PropertyKind, Value,
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
        LegacyPropertySpec::interpolated(PropertyKind::Number)
    }

    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Number(number) => Some(Self(vec![*number])),
            Value::Array(items) => items
                .iter()
                .map(Value::as_number)
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
        LegacyPropertySpec::interpolated(PropertyKind::Color)
    }

    fn from_value(value: &Value) -> Option<Self> {
        match value {
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
        value.as_number().map(|number| number as f32)
    }
}

impl PropertyValue for f64 {
    fn spec() -> LegacyPropertySpec {
        LegacyPropertySpec::interpolated(PropertyKind::Number)
    }

    fn from_value(value: &Value) -> Option<Self> {
        value.as_number()
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
            Value::Array(items) if items.len() == 3 => Some([
                items[0].as_number()?,
                items[1].as_number()?,
                items[2].as_number()?,
            ]),
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
}

/// A property value the engine could not parse.
#[derive(Debug, Clone)]
pub struct UnsupportedProperty {
    /// The JSON as written in the style.
    pub source: serde_json::Value,
    /// Why it was rejected.
    pub error: String,
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

impl<T: PropertyValue> StyleProperty<T> {
    /// Parses a property value: a constant, a legacy function object or an expression.
    pub fn parse(json: &serde_json::Value) -> Self {
        if let Some(constant) = T::from_literal(json) {
            return Self::Constant(constant);
        }
        match Expression::parse_property(json, &T::spec()) {
            Ok(Expression::Literal(value)) | Ok(Expression::Folded { value, .. }) => {
                match T::from_value(&value) {
                    Some(constant) => Self::Constant(constant),
                    None => Self::Unsupported(Arc::new(UnsupportedProperty {
                        source: json.clone(),
                        error: format!(
                            "expected {}, found {}",
                            T::spec().expected_type(),
                            value.type_of()
                        ),
                    })),
                }
            }
            Ok(expression) => Self::Expression(Arc::new(PropertyExpression {
                source: json.clone(),
                expression,
            })),
            Err(error) => {
                tracing::error!(source = %json, %error, "unsupported style property value");
                Self::Unsupported(Arc::new(UnsupportedProperty {
                    source: json.clone(),
                    error: error.to_string(),
                }))
            }
        }
    }

    /// Evaluates the property; `None` when the expression fails or was unsupported, so the
    /// caller applies the specification default.
    pub fn evaluate(&self, context: &EvaluationContext) -> Option<T> {
        match self {
            Self::Constant(value) => Some(value.clone()),
            Self::Expression(property) => {
                let value = property.expression.evaluate(context).ok()?;
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
