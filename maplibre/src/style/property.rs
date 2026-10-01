//! Style property values: a constant, or an expression evaluated per feature and per zoom.

use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::style::expression::{
    convert_token_string, EvaluationContext, Expression, FeatureProperties, LegacyPropertySpec,
    ParseError, PropertyKind, Value,
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

/// A run of a formatted text that has its own size, colour or font.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextSection {
    /// How many characters the run holds.
    pub length: usize,
    /// Factor on the layout size; `None` leaves the size unchanged.
    pub scale: Option<f32>,
    /// Straight RGBA text colour that replaces the layer's.
    pub color: Option<[f32; 4]>,
    /// Font stack, comma-joined, that replaces the layer's.
    pub font: Option<String>,
    /// The image the run is, drawn in the line of text in place of a character.
    pub image: Option<String>,
}

/// The text of a symbol: a `{token}` template, a literal, or an expression producing text, in
/// one section or, for a `format` expression, several.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextField(pub String, pub Vec<TextSection>);

/// A text field with sections is written in the lowered form `decode` reads back, so a layer
/// that is serialized and parsed again keeps its sections.
impl Serialize for TextField {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.encode())
    }
}

impl TextField {
    /// A text that is all one section.
    pub fn plain(text: impl Into<String>) -> Self {
        Self(text.into(), Vec::new())
    }

    fn encode(&self) -> String {
        use crate::style::expression::{FORMATTED_START, FORMAT_FIELD, FORMAT_SECTION};

        if self.1.is_empty() {
            return self.0.clone();
        }
        let mut encoded = FORMATTED_START.to_string();
        let mut characters = self.0.chars();
        for section in &self.1 {
            let content: String = characters.by_ref().take(section.length).collect();
            let color = section.color.map_or_else(String::new, |[r, g, b, a]| {
                csscolorparser::Color::new(r.into(), g.into(), b.into(), a.into()).to_hex_string()
            });
            let font = section.font.as_ref().map_or_else(String::new, |font| {
                serde_json::to_string(&font.split(',').collect::<Vec<_>>()).unwrap_or_default()
            });
            encoded.push(FORMAT_SECTION);
            encoded.push_str(
                &section
                    .scale
                    .map_or_else(String::new, |scale| scale.to_string()),
            );
            for field in [&color, &font, &content] {
                encoded.push(FORMAT_FIELD);
                encoded.push_str(field);
            }
            encoded.push(FORMAT_FIELD);
            encoded.push_str(section.image.as_deref().unwrap_or_default());
        }
        encoded
    }

    /// Reads the text a `format` expression lowered to, or a plain string as it is.
    fn decode(text: &str) -> Self {
        use crate::style::expression::{FORMATTED_START, FORMAT_FIELD, FORMAT_SECTION};

        let Some(body) = text.strip_prefix(FORMATTED_START) else {
            return Self::plain(text);
        };
        let mut plain = String::new();
        let mut sections = Vec::new();
        for section in body
            .split(FORMAT_SECTION)
            .filter(|section| !section.is_empty())
        {
            let mut fields = section.splitn(5, FORMAT_FIELD);
            let (scale, color, font, content, image) = (
                fields.next().unwrap_or_default(),
                fields.next().unwrap_or_default(),
                fields.next().unwrap_or_default(),
                fields.next().unwrap_or_default(),
                fields.next().unwrap_or_default(),
            );
            plain.push_str(content);
            sections.push(TextSection {
                length: content.chars().count(),
                scale: scale.parse().ok(),
                color: csscolorparser::parse(color)
                    .ok()
                    .map(|color| color.to_array().map(|channel| channel as f32)),
                font: serde_json::from_str::<Vec<String>>(font)
                    .ok()
                    .filter(|fonts| !fonts.is_empty())
                    .map(|fonts| fonts.join(",")),
                image: (!image.is_empty()).then(|| image.to_owned()),
            });
        }
        if sections.iter().all(|section| {
            section.scale.is_none()
                && section.color.is_none()
                && section.font.is_none()
                && section.image.is_none()
        }) {
            return Self::plain(plain);
        }
        Self(plain, sections)
    }
}

impl PropertyValue for TextField {
    fn spec() -> LegacyPropertySpec {
        LegacyPropertySpec {
            kind: PropertyKind::String,
            interpolated: false,
            default: None,
            tokens: true,
        }
    }

    fn from_value(value: &Value) -> Option<Self> {
        value.as_str().map(Self::decode)
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

impl<T: PropertyValue> StyleProperty<T> {
    /// Parses a property value: a constant, a legacy function object or an expression.
    pub fn parse(json: &serde_json::Value) -> Self {
        if let Some(constant) = T::from_literal(json) {
            return Self::Constant(constant);
        }
        let spec = T::spec();
        // A `{token}` string reads feature properties, as GL JS lowers it before parsing.
        let lowered = match json.as_str() {
            Some(text) if spec.tokens => convert_token_string(text),
            _ => json.clone(),
        };
        match Expression::parse_property(&lowered, &spec) {
            Ok(Expression::Literal(value)) | Ok(Expression::Folded { value, .. }) => {
                match T::from_value(&value) {
                    Some(constant) => Self::Constant(constant),
                    None => Self::Unsupported(Arc::new(UnsupportedProperty {
                        source: json.clone(),
                        error: ParseError {
                            key: String::new(),
                            message: format!(
                                "expected {}, found {}",
                                spec.expected_type(),
                                value.type_of()
                            ),
                        },
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
                    error,
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
