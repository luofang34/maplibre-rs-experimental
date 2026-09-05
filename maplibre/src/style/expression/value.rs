//! Typed values an expression consumes and produces, with the GL JS semantics of `typeof`,
//! strict equality and string conversion.

use std::{collections::BTreeMap, fmt};

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
        let color = csscolorparser::parse(text).ok()?;
        Some(Self::new(color.r, color.g, color.b, color.a))
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
        let channel = |value: f64| (value * 255.0).round();
        format!(
            "rgba({},{},{},{})",
            channel(self.r),
            channel(self.g),
            channel(self.b),
            js_number(self.a)
        )
    }
}

impl From<Color> for csscolorparser::Color {
    fn from(color: Color) -> Self {
        csscolorparser::Color::new(color.r, color.g, color.b, color.a)
    }
}

impl From<csscolorparser::Color> for Color {
    fn from(color: csscolorparser::Color) -> Self {
        Self::new(color.r, color.g, color.b, color.a)
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
    Object(BTreeMap<String, Value>),
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
            Self::Number(number) => {
                if number.fract() == 0.0 && number.abs() < 9007199254740992.0 {
                    serde_json::Value::Number((*number as i64).into())
                } else {
                    serde_json::Number::from_f64(*number)
                        .map_or(serde_json::Value::Null, serde_json::Value::Number)
                }
            }
            Self::String(text) => serde_json::Value::String(text.clone()),
            Self::Color(color) => serde_json::Value::Array(
                color
                    .premultiplied()
                    .iter()
                    .map(|component| Self::Number(*component).to_json())
                    .collect(),
            ),
            Self::Array(items) => {
                serde_json::Value::Array(items.iter().map(Self::to_json).collect())
            }
            Self::Object(members) => serde_json::Value::Object(
                members
                    .iter()
                    .map(|(key, value)| (key.clone(), value.to_json()))
                    .collect(),
            ),
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
            Self::Array(items) => {
                let mut item_type: Option<Type> = None;
                for item in items {
                    let candidate = item.type_of();
                    match &item_type {
                        None => item_type = Some(candidate),
                        Some(current) if *current == candidate => {}
                        Some(_) => {
                            item_type = Some(Type::Value);
                            break;
                        }
                    }
                }
                Type::array(item_type.unwrap_or(Type::Value), Some(items.len()))
            }
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
            Self::Color(_) | Self::Array(_) | Self::Object(_) => true,
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
            Self::Array(_) | Self::Object(_) => self.to_json().to_string(),
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

/// Formats a number the way JavaScript's `String(number)` does.
pub fn js_number(number: f64) -> String {
    if number.is_nan() {
        return "NaN".to_string();
    }
    if number.is_infinite() {
        return if number > 0.0 {
            "Infinity"
        } else {
            "-Infinity"
        }
        .to_string();
    }
    if number == 0.0 {
        return "0".to_string();
    }
    let magnitude = number.abs();
    if (1e-6..1e21).contains(&magnitude) {
        return format!("{number}");
    }
    let scientific = format!("{number:e}");
    match scientific.split_once('e') {
        Some((mantissa, exponent)) if !exponent.starts_with('-') => {
            format!("{mantissa}e+{exponent}")
        }
        _ => scientific,
    }
}

/// The static type of an expression or the dynamic type of a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Type {
    /// `null`.
    Null,
    /// A number.
    Number,
    /// A string.
    String,
    /// A boolean.
    Boolean,
    /// A colour.
    Color,
    /// An object.
    Object,
    /// Any value; the type of a property read or an untyped branch.
    Value,
    /// An array, with an item type and, when known, a length.
    Array {
        /// Type of every item.
        item: Box<Type>,
        /// Number of items, when the type fixes it.
        length: Option<usize>,
    },
}

impl Type {
    /// An array type.
    pub fn array(item: Type, length: Option<usize>) -> Self {
        Self::Array {
            item: Box::new(item),
            length,
        }
    }

    /// Whether a value of this type is acceptable where `expected` is required, as GL JS
    /// `checkSubtype` decides.
    pub fn is_subtype_of(&self, expected: &Type) -> bool {
        match (expected, self) {
            (
                Type::Array {
                    item: expected_item,
                    length: expected_length,
                },
                Type::Array { item, length },
            ) => {
                let items_fit = (*length == Some(0) && **item == Type::Value)
                    || item.is_subtype_of(expected_item);
                items_fit && expected_length.is_none_or(|expected| Some(expected) == *length)
            }
            (Type::Array { .. }, _) => false,
            (Type::Value, _) => true,
            (expected, actual) => expected == actual,
        }
    }

    /// The name GL JS prints, such as `array<number, 3>`.
    pub fn name(&self) -> String {
        match self {
            Self::Null => "null".to_string(),
            Self::Number => "number".to_string(),
            Self::String => "string".to_string(),
            Self::Boolean => "boolean".to_string(),
            Self::Color => "color".to_string(),
            Self::Object => "object".to_string(),
            Self::Value => "value".to_string(),
            Self::Array { item, length } => match (item.as_ref(), length) {
                (_, Some(length)) => format!("array<{}, {length}>", item.name()),
                (Type::Value, None) => "array".to_string(),
                (item, None) => format!("array<{}>", item.name()),
            },
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.name())
    }
}
