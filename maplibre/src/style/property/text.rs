//! The text and image values of symbol properties: text with sections, formatted text and the
//! names of images.

use serde::{Serialize, Serializer};

use super::PropertyValue;
use crate::style::expression::{LegacyPropertySpec, PropertyKind, Value};

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

    /// The text and sections of formatted text. An image section stands in the text as one
    /// [`crate::style::expression::FORMAT_IMAGE`] character, whether or not the map holds it
    /// yet; layout draws it once it does.
    pub fn from_formatted(formatted: &crate::style::expression::Formatted) -> Self {
        let mut plain = String::new();
        let mut sections = Vec::with_capacity(formatted.sections.len());
        for section in &formatted.sections {
            let text = match &section.image {
                Some(_) => crate::style::expression::FORMAT_IMAGE.to_string(),
                None => section.text.clone(),
            };
            plain.push_str(&text);
            sections.push(TextSection {
                length: text.chars().count(),
                scale: section.scale.map(|scale| scale as f32),
                color: section
                    .color
                    .map(|color| color.straight().map(|channel| channel as f32)),
                font: section
                    .font
                    .as_ref()
                    .filter(|fonts| !fonts.is_empty())
                    .map(|fonts| fonts.join(",")),
                image: section.image.as_ref().map(|image| image.name.clone()),
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

    /// Reads the written form of a text with sections, or a plain string as it is.
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
        match value {
            Value::String(text) => Some(Self::decode(text)),
            _ => None,
        }
    }
}

/// The image an image property names: `icon-image` or a pattern. A string names its image,
/// and an `image` expression its image by name, whether or not the map holds it yet; where it
/// does not, a `coalesce` around the expression chooses a fallback first.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ImageName(pub String);

impl PropertyValue for ImageName {
    fn spec() -> LegacyPropertySpec {
        LegacyPropertySpec {
            kind: PropertyKind::ResolvedImage,
            interpolated: false,
            default: None,
            // `icon-image` may name its image with `{token}` references to feature properties.
            tokens: true,
        }
    }

    fn from_value(value: &Value) -> Option<Self> {
        image_name(value).map(Self)
    }
}

/// The image a pattern property names: like [`ImageName`], without `{token}` references, which
/// the specification gives patterns none of.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct PatternName(pub String);

impl PropertyValue for PatternName {
    fn spec() -> LegacyPropertySpec {
        LegacyPropertySpec {
            kind: PropertyKind::ResolvedImage,
            interpolated: false,
            default: None,
            tokens: false,
        }
    }

    fn from_value(value: &Value) -> Option<Self> {
        image_name(value).map(Self)
    }
}

/// The name an image property's value gives.
fn image_name(value: &Value) -> Option<String> {
    match value {
        Value::Image(image) => Some(image.name.clone()),
        Value::String(name) => Some(name.clone()),
        _ => None,
    }
}

/// The text of `text-field`: a [`TextField`] evaluated as formatted text, so the sections and
/// images a `case`, `match` or `coalesce` inside a `format` chooses stay sections and images.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FormattedText(pub TextField);

/// Text with sections is written as the `format` expression that makes it, so a layer that is
/// serialized and parsed again keeps its sections and images.
impl Serialize for FormattedText {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let TextField(text, sections) = &self.0;
        if sections.is_empty() {
            return serializer.serialize_str(text);
        }
        let mut format = vec![serde_json::json!("format")];
        let mut characters = text.chars();
        for section in sections {
            let content: String = characters.by_ref().take(section.length).collect();
            let Some(image) = &section.image else {
                let mut options = serde_json::Map::new();
                if let Some(scale) = section.scale {
                    options.insert("font-scale".into(), serde_json::json!(scale));
                }
                if let Some([r, g, b, a]) = section.color {
                    let color = csscolorparser::Color::new(r.into(), g.into(), b.into(), a.into());
                    options.insert("text-color".into(), color.to_hex_string().into());
                }
                if let Some(font) = &section.font {
                    let fonts: Vec<&str> = font.split(',').collect();
                    options.insert("text-font".into(), serde_json::json!(["literal", fonts]));
                }
                format.extend([
                    serde_json::json!(content),
                    serde_json::Value::Object(options),
                ]);
                continue;
            };
            format.push(serde_json::json!(["image", image]));
        }
        serde_json::Value::Array(format).serialize(serializer)
    }
}

impl PropertyValue for FormattedText {
    fn spec() -> LegacyPropertySpec {
        LegacyPropertySpec {
            kind: PropertyKind::Formatted,
            interpolated: false,
            default: None,
            tokens: true,
        }
    }

    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Formatted(formatted) => Some(Self(TextField::from_formatted(formatted))),
            // The written form of a text field with sections reads back into them.
            Value::String(text) => Some(Self(TextField::decode(text))),
            _ => None,
        }
    }
}
