//! Static expression types (`Type`) and GL JS subtype / `typeof` naming rules.

use std::fmt;

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
    /// A string comparison rule.
    Collator,
    /// An image name and whether the map holds it.
    ResolvedImage,
    /// Text in sections.
    Formatted,
    /// Directional `[top, right, bottom, left]` box padding (`padding`).
    Padding,
    /// Variable-length numeric list (`numberArray`).
    NumberArray,
    /// Variable-length colour list (`colorArray`).
    ColorArray,
    /// Map projection name or blend (`projectionDefinition`).
    ProjectionDefinition,
    /// Alternating anchor and `[x, y]` offset pairs (`variableAnchorOffsetCollection`).
    VariableAnchorOffsetCollection,
    /// Bottom type produced by `["error", ...]` expressions; subtypes every type.
    Error,
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
            (_, Type::Error) => true,
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

    /// The name GL JS prints, such as `array<number, 3>` or `padding`.
    pub fn name(&self) -> String {
        match self {
            Self::Null => "null".to_string(),
            Self::Number => "number".to_string(),
            Self::String => "string".to_string(),
            Self::Boolean => "boolean".to_string(),
            Self::Color => "color".to_string(),
            Self::Object => "object".to_string(),
            Self::Collator => "collator".to_string(),
            Self::ResolvedImage => "resolvedImage".to_string(),
            Self::Formatted => "formatted".to_string(),
            Self::Padding => "padding".to_string(),
            Self::NumberArray => "numberArray".to_string(),
            Self::ColorArray => "colorArray".to_string(),
            Self::ProjectionDefinition => "projectionDefinition".to_string(),
            Self::VariableAnchorOffsetCollection => "variableAnchorOffsetCollection".to_string(),
            Self::Error => "error".to_string(),
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
