//! Typed values an expression consumes and produces, with the GL JS semantics of `typeof`,
//! strict equality and string conversion.

use collation::Collation;
use rustc_hash::FxHashMap;

pub(crate) mod collation;
mod color;
mod composite;
mod convert;
mod formatted;
mod types;

pub use color::{interpolate_number, ColorSpace};
pub use composite::{
    ColorArray, NumberArray, Padding, ProjectionDefinition, VariableAnchorOffsetCollection,
};
pub use formatted::{Formatted, FormattedSection, ResolvedImage};
pub use types::Type;

/// A colour with straight (not premultiplied) components in `0..=1`.
///
/// GL JS stores colours premultiplied; [`Color::premultiplied`] gives that form, which is also
/// how the conformance suite writes its expected colours.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    /// Red, straight.
    pub r: f64,
    /// Green, straight.
    pub g: f64,
    /// Blue, straight.
    pub b: f64,
    /// Alpha.
    pub a: f64,
}

impl Color {
    /// A colour from straight components.
    pub const fn new(r: f64, g: f64, b: f64, a: f64) -> Self {
        Self { r, g, b, a }
    }

    /// Parses any CSS colour string, as GL JS `Color.parse` does.
    pub fn parse(text: &str) -> Option<Self> {
        let [r, g, b, a] = crate::util::css_color::parse_css_color(text)?;
        Some(Self::new(
            f64::from(r),
            f64::from(g),
            f64::from(b),
            f64::from(a),
        ))
    }

    /// Components with red, green and blue multiplied by alpha, as GL JS stores them.
    pub fn premultiplied(&self) -> [f64; 4] {
        [self.r * self.a, self.g * self.a, self.b * self.a, self.a]
    }

    /// Straight components, red, green and blue in `0..=1`.
    pub fn straight(&self) -> [f64; 4] {
        [self.r, self.g, self.b, self.a]
    }

    /// The `rgba(r,g,b,a)` form GL JS prints, with channels in `0..=255`.
    pub fn css(&self) -> String {
        format!(
            "rgba({},{},{},{})",
            color_channel_byte(self.r),
            color_channel_byte(self.g),
            color_channel_byte(self.b),
            js_number(self.a)
        )
    }
}

#[inline]
fn color_channel_byte(value: f64) -> f64 {
    (value * 255.0).round()
}

fn color_to_json(color: Color) -> serde_json::Value {
    serde_json::Value::Array(
        color
            .premultiplied()
            .iter()
            .map(|component| number_to_json(*component))
            .collect(),
    )
}

fn infer_array_item_type(items: &[Value]) -> Type {
    let mut item_type: Option<Type> = None;
    for item in items {
        let candidate = item.type_of();
        match &item_type {
            None => item_type = Some(candidate),
            Some(current) if *current == candidate => {}
            Some(_) => return Type::Value,
        }
    }
    item_type.unwrap_or(Type::Value)
}

impl From<Color> for csscolorparser::Color {
    fn from(color: Color) -> Self {
        csscolorparser::Color::new(
            color.r as f32,
            color.g as f32,
            color.b as f32,
            color.a as f32,
        )
    }
}

impl From<csscolorparser::Color> for Color {
    fn from(color: csscolorparser::Color) -> Self {
        Self::new(
            f64::from(color.r),
            f64::from(color.g),
            f64::from(color.b),
            f64::from(color.a),
        )
    }
}

/// A value at run time.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// `null`, also what a missing property evaluates to.
    Null,
    /// A boolean.
    Bool(bool),
    /// A number.
    Number(f64),
    /// A string.
    String(String),
    /// A colour.
    Color(Color),
    /// An array of values.
    Array(Vec<Value>),
    /// An object with string keys.
    Object(FxHashMap<String, Value>),
    /// How strings compare, made by `collator`.
    Collator(Collation),
    /// An image an `image` expression names.
    Image(ResolvedImage),
    /// Text in sections, made by `format`.
    Formatted(Formatted),
    /// Directional `[top, right, bottom, left]` box padding (`padding`).
    Padding(Padding),
    /// Variable-length numeric list (`numberArray`).
    NumberArray(NumberArray),
    /// Variable-length colour list (`colorArray`).
    ColorArray(ColorArray),
    /// Map projection name or blend (`projectionDefinition`).
    ProjectionDefinition(ProjectionDefinition),
    /// Alternating anchor and `[x, y]` offset pairs (`variableAnchorOffsetCollection`).
    VariableAnchorOffsetCollection(VariableAnchorOffsetCollection),
}

impl Value {
    /// The value a JSON document holds; nothing is coerced, so a colour stays a string.
    pub fn from_json(json: &serde_json::Value) -> Self {
        match json {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(flag) => Self::Bool(*flag),
            serde_json::Value::Number(number) => Self::Number(number.as_f64().unwrap_or(f64::NAN)),
            serde_json::Value::String(text) => Self::String(text.clone()),
            serde_json::Value::Array(items) => {
                Self::Array(items.iter().map(Self::from_json).collect())
            }
            serde_json::Value::Object(members) => Self::Object(
                members
                    .iter()
                    .map(|(key, value)| (key.clone(), Self::from_json(value)))
                    .collect(),
            ),
        }
    }

    /// The JSON form; a colour becomes its premultiplied `[r, g, b, a]`.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Null => serde_json::Value::Null,
            Self::Bool(flag) => serde_json::Value::Bool(*flag),
            Self::Number(number) => number_to_json(*number),
            Self::String(text) => serde_json::Value::String(text.clone()),
            Self::Color(color) => color_to_json(*color),
            Self::Array(items) => {
                serde_json::Value::Array(items.iter().map(Self::to_json).collect())
            }
            Self::Object(members) => serde_json::Value::Object(
                members
                    .iter()
                    .map(|(key, value)| (key.clone(), value.to_json()))
                    .collect(),
            ),
            Self::Collator(_) => serde_json::Value::Null,
            Self::Image(image) => image.to_json(),
            Self::Formatted(formatted) => formatted.to_json(),
            Self::Padding(padding) => padding.to_json(),
            Self::NumberArray(numbers) => numbers.to_json(),
            Self::ColorArray(colors) => colors.to_json(),
            Self::ProjectionDefinition(projection) => projection.to_json(),
            Self::VariableAnchorOffsetCollection(offsets) => offsets.to_json(),
        }
    }

    /// The type GL JS `typeof` reports; an array's item type and length are inferred.
    pub fn type_of(&self) -> Type {
        match self {
            Self::Null => Type::Null,
            Self::Bool(_) => Type::Boolean,
            Self::Number(_) => Type::Number,
            Self::String(_) => Type::String,
            Self::Color(_) => Type::Color,
            Self::Object(_) => Type::Object,
            Self::Collator(_) => Type::Collator,
            Self::Image(_) => Type::ResolvedImage,
            Self::Formatted(_) => Type::Formatted,
            Self::Padding(_) => Type::Padding,
            Self::NumberArray(_) => Type::NumberArray,
            Self::ColorArray(_) => Type::ColorArray,
            Self::ProjectionDefinition(_) => Type::ProjectionDefinition,
            Self::VariableAnchorOffsetCollection(_) => Type::VariableAnchorOffsetCollection,
            Self::Array(items) => Type::array(infer_array_item_type(items), Some(items.len())),
        }
    }

    /// The number, if the value is one.
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Self::Number(number) => Some(*number),
            _ => None,
        }
    }

    /// The string, if the value is one.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(text) => Some(text),
            _ => None,
        }
    }

    /// The boolean, if the value is one.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(flag) => Some(*flag),
            _ => None,
        }
    }

    /// Whether the value is `null`.
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// JavaScript truthiness, what `to-boolean` and `case` conditions use.
    pub fn truthy(&self) -> bool {
        match self {
            Self::Null => false,
            Self::Bool(flag) => *flag,
            Self::Number(number) => *number != 0.0 && !number.is_nan(),
            Self::String(text) => !text.is_empty(),
            Self::Color(_)
            | Self::Array(_)
            | Self::Object(_)
            | Self::Collator(_)
            | Self::Image(_)
            | Self::Formatted(_)
            | Self::Padding(_)
            | Self::NumberArray(_)
            | Self::ColorArray(_)
            | Self::ProjectionDefinition(_)
            | Self::VariableAnchorOffsetCollection(_) => true,
        }
    }

    /// The string GL JS `to-string` and `concat` produce.
    pub fn to_display_string(&self) -> String {
        match self {
            Self::Null => String::new(),
            Self::Bool(flag) => flag.to_string(),
            Self::Number(number) => js_number(*number),
            Self::String(text) => text.clone(),
            Self::Color(color) => color.css(),
            Self::ProjectionDefinition(ProjectionDefinition::Named(name)) => name.clone(),
            Self::Array(_)
            | Self::Object(_)
            | Self::Padding(_)
            | Self::NumberArray(_)
            | Self::ColorArray(_)
            | Self::ProjectionDefinition(_)
            | Self::VariableAnchorOffsetCollection(_) => {
                crate::util::json::json_value_to_string(&self.to_json())
            }
            Self::Collator(_) => String::new(),
            Self::Image(image) => image.name.clone(),
            Self::Formatted(formatted) => formatted.text(),
        }
    }
}

impl From<f64> for Value {
    fn from(number: f64) -> Self {
        Self::Number(number)
    }
}

impl From<bool> for Value {
    fn from(flag: bool) -> Self {
        Self::Bool(flag)
    }
}

impl From<&str> for Value {
    fn from(text: &str) -> Self {
        Self::String(text.to_string())
    }
}

impl From<String> for Value {
    fn from(text: String) -> Self {
        Self::String(text)
    }
}

impl From<Color> for Value {
    fn from(color: Color) -> Self {
        Self::Color(color)
    }
}

pub(super) fn number_to_json(number: f64) -> serde_json::Value {
    if number.fract() == 0.0 && number.abs() < 9007199254740992.0 {
        serde_json::Value::Number((number as i64).into())
    } else {
        serde_json::Number::from_f64(number)
            .map_or(serde_json::Value::Null, serde_json::Value::Number)
    }
}

pub use crate::util::json::js_number;
