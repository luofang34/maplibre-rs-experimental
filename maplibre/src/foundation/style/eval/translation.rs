//! The `*-translate` properties, which may change with zoom.
use serde::{Deserialize, Serialize};

use crate::style::{
    expression::{LegacyPropertySpec, PropertyKind, Value},
    property::{PropertyValue, StyleProperty},
};

/// Two numbers, as the specification's `[x, y]` properties hold them.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Pair(pub [f64; 2]);

impl PropertyValue for Pair {
    fn spec() -> LegacyPropertySpec {
        LegacyPropertySpec::interpolated(PropertyKind::Array {
            item: Box::new(PropertyKind::Number),
            length: Some(2),
        })
    }

    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Array(items) => match items.as_slice() {
                [x, y] => Some(Self([x.as_number()?, y.as_number()?])),
                _ => None,
            },
            _ => None,
        }
    }

    fn from_literal(json: &serde_json::Value) -> Option<Self> {
        Self::from_json(json)
    }
}

impl Pair {
    /// A pair written out as a JSON array of two numbers.
    pub fn from_json(json: &serde_json::Value) -> Option<Self> {
        match json.as_array()?.as_slice() {
            [x, y] => Some(Self([x.as_f64()?, y.as_f64()?])),
            _ => None,
        }
    }
}

/// A translation in screen pixels: the same at every zoom, or a function of zoom.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Translation(StyleProperty<Pair>);

impl Translation {
    /// Parses a translation property directly from a JSON [`serde_json::Value`].
    pub(crate) fn from_json_value(value: &serde_json::Value) -> Self {
        Self(StyleProperty::parse(value))
    }

    /// The underlying `[x, y]` style property.
    pub(crate) fn property(&self) -> &StyleProperty<Pair> {
        &self.0
    }

    /// The `[x, y]` translation at `zoom`; none is no translation.
    pub fn at_zoom(&self, zoom: f64) -> [f32; 2] {
        self.0
            .evaluate_at_zoom(zoom)
            .map_or([0.0; 2], |Pair([x, y])| [x as f32, y as f32])
    }
}

impl From<[f32; 2]> for Translation {
    fn from([x, y]: [f32; 2]) -> Self {
        Self(StyleProperty::Constant(Pair([f64::from(x), f64::from(y)])))
    }
}

/// Coordinate frame used by fill and line paint translations.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum TranslateAnchor {
    /// Translation follows map axes.
    #[default]
    #[serde(rename = "map")]
    Map,
    /// Translation follows viewport axes.
    #[serde(rename = "viewport")]
    Viewport,
}

/// The translation a layer asks for at `zoom`; none is no translation.
pub fn translation_at(translation: Option<&Translation>, zoom: f64) -> [f32; 2] {
    translation.map_or([0.0; 2], |translation| translation.at_zoom(zoom))
}
