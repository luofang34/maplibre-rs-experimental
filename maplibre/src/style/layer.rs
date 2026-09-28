//! Vector tile layer drawing utilities.

use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
};

use cint::{Alpha, EncodedSrgb};
use serde::{Deserialize, Serialize};

use crate::style::{
    circle::CirclePaint,
    hillshade::{ColorReliefPaint, HillshadePaint},
};

pub use crate::style::property::{PropertyValue, StyleProperty, TextField};

mod paint;
mod serialization;
pub use paint::{
    BackgroundPaint, FillPaint, LinePaint, RasterPaint, RasterResampling, SymbolPaint,
    TranslateAnchor,
};
pub use serialization::LayerProperties;

/// The different types of paints.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", content = "paint")]
pub enum LayerPaint {
    #[serde(rename = "background")]
    Background(BackgroundPaint),
    #[serde(rename = "line")]
    Line(LinePaint),
    #[serde(rename = "fill")]
    Fill(FillPaint),
    #[serde(rename = "raster")]
    Raster(RasterPaint),
    #[serde(rename = "hillshade")]
    Hillshade(HillshadePaint),
    #[serde(rename = "color-relief")]
    ColorRelief(ColorReliefPaint),
    #[serde(rename = "symbol")]
    Symbol(SymbolPaint),
    #[serde(rename = "circle")]
    Circle(CirclePaint),
}

impl LayerPaint {
    /// The opacity property multiplied into the layer's colour, when the layer type has one.
    pub fn opacity(&self) -> Option<StyleProperty<f32>> {
        match self {
            LayerPaint::Fill(paint) => paint.fill_opacity.clone(),
            LayerPaint::Line(paint) => paint.line_opacity.clone(),
            LayerPaint::Circle(paint) => paint.circle_opacity.clone(),
            LayerPaint::ColorRelief(paint) => paint.color_relief_opacity.clone(),
            LayerPaint::Background(_)
            | LayerPaint::Raster(_)
            | LayerPaint::Hillshade(_)
            | LayerPaint::Symbol(_) => None,
        }
    }

    pub fn get_color(&self) -> Option<Alpha<EncodedSrgb<f32>>> {
        match self {
            LayerPaint::Background(paint) => paint.background_color.as_ref().and_then(|property| {
                if let StyleProperty::Constant(color) = property {
                    Some(color.clone().into())
                } else {
                    None // Expression types have no single static color
                }
            }),
            LayerPaint::Line(paint) => paint.line_color.as_ref().and_then(|property| {
                if let StyleProperty::Constant(color) = property {
                    Some(color.clone().into())
                } else {
                    None
                }
            }),
            LayerPaint::Fill(paint) => paint.fill_color.as_ref().and_then(|property| {
                if let StyleProperty::Constant(color) = property {
                    Some(color.clone().into())
                } else {
                    None
                }
            }),
            LayerPaint::Circle(paint) => paint.circle_color.as_ref().and_then(|property| {
                if let StyleProperty::Constant(color) = property {
                    Some(color.clone().into())
                } else {
                    None
                }
            }),
            LayerPaint::Raster(_)
            | LayerPaint::Hillshade(_)
            | LayerPaint::ColorRelief(_)
            | LayerPaint::Symbol(_) => None,
        }
    }
}

/// Whether a layer is drawn at all, the `layout.visibility` property.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum LayerVisibility {
    /// The layer is drawn.
    #[default]
    #[serde(rename = "visible")]
    Visible,
    /// The layer is neither laid out nor drawn.
    #[serde(rename = "none")]
    None,
}

/// Stores all the styles for a specific layer.
#[derive(Debug, Clone)]
pub struct StyleLayer {
    pub index: u32,
    pub id: String,
    pub type_: String,
    pub filter: Option<serde_json::Value>,
    pub maxzoom: Option<u8>,
    pub minzoom: Option<u8>,
    pub metadata: Option<HashMap<String, String>>,
    pub paint: Option<LayerPaint>,
    pub source: Option<String>,
    pub source_layer: Option<String>,
    /// Whether the layer is drawn at all.
    pub visibility: LayerVisibility,
    /// Properties without a typed representation, retained for diagnostics and round trips.
    pub unrecognized: LayerProperties,
}

impl StyleLayer {
    /// Whether the layer is switched off by its `layout.visibility`, at every zoom.
    pub fn is_hidden(&self) -> bool {
        self.visibility == LayerVisibility::None
    }

    /// Returns whether the layer is drawn at a continuous zoom: it is not hidden and the zoom
    /// lies in its range, using the style-spec rule that `minzoom` is inclusive and `maxzoom`
    /// exclusive.
    pub fn is_visible_at(&self, zoom: f64) -> bool {
        !self.is_hidden()
            && self
                .minzoom
                .is_none_or(|minzoom| zoom >= f64::from(minzoom))
            && self.maxzoom.is_none_or(|maxzoom| zoom < f64::from(maxzoom))
    }
}

impl Eq for StyleLayer {}
impl PartialEq for StyleLayer {
    fn eq(&self, other: &Self) -> bool {
        self.id.eq(&other.id)
    }
}

impl Hash for StyleLayer {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state)
    }
}

impl Default for StyleLayer {
    fn default() -> Self {
        Self {
            index: 0,
            id: "id".to_string(),
            type_: "background".to_string(),
            filter: None,
            maxzoom: None,
            minzoom: None,
            metadata: None,
            paint: None,
            source: None,
            source_layer: Some("does not exist".to_string()),
            visibility: LayerVisibility::Visible,
            unrecognized: Default::default(),
        }
    }
}

#[cfg(test)]
mod tests;
