//! Layer identity, visibility and typed paint properties.

use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
};

use cint::{Alpha, EncodedSrgb};
use serde::{Deserialize, Serialize};

pub use crate::style::property::{PropertyValue, StyleProperty, TextField};
use crate::style::{
    circle::CirclePaint,
    heatmap::HeatmapPaint,
    hillshade::{ColorReliefPaint, HillshadePaint},
};

mod fill_outline;
mod paint;
mod serialization;
pub use paint::{
    BackgroundPaint, FillExtrusionPaint, FillPaint, LinePaint, RasterPaint, RasterResampling,
    SymbolPaint, TranslateAnchor,
};
pub use serialization::LayerProperties;

/// Typed properties selected by a style layer's rendering type.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", content = "paint")]
pub enum LayerPaint {
    /// Background color beneath source-backed layers.
    #[serde(rename = "background")]
    Background(BackgroundPaint),
    /// Stroked paths with width, opacity, translation and dash properties.
    #[serde(rename = "line")]
    Line(LinePaint),
    /// Filled polygon color, opacity and translation.
    #[serde(rename = "fill")]
    Fill(FillPaint),
    /// Polygons raised to a height, with lit walls and roof.
    #[serde(rename = "fill-extrusion")]
    FillExtrusion(FillExtrusionPaint),
    /// Image sampling and adjustment properties retained from the style.
    #[serde(rename = "raster")]
    Raster(RasterPaint),
    /// Elevation-dependent illumination and shadow colors.
    #[serde(rename = "hillshade")]
    Hillshade(HillshadePaint),
    /// Color ramp evaluated against decoded elevation.
    #[serde(rename = "color-relief")]
    ColorRelief(ColorReliefPaint),
    /// Text and icon appearance and layout properties.
    #[serde(rename = "symbol")]
    Symbol(SymbolPaint),
    /// Point radius, fill, stroke and alignment properties.
    #[serde(rename = "circle")]
    Circle(CirclePaint),
    /// Point density radius, weight, intensity, opacity and colour ramp.
    #[serde(rename = "heatmap")]
    Heatmap(HeatmapPaint),
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
            | LayerPaint::FillExtrusion(_)
            | LayerPaint::Raster(_)
            | LayerPaint::Hillshade(_)
            | LayerPaint::Symbol(_)
            | LayerPaint::Heatmap(_) => None,
        }
    }

    /// Returns a constant base color in encoded sRGB with straight alpha.
    /// Expressions, absent colors and paint families without one base color return `None`.
    /// Layer opacity is not multiplied into this value.
    pub fn get_color(&self) -> Option<Alpha<EncodedSrgb<f32>>> {
        let property = match self {
            Self::Background(paint) => &paint.background_color,
            Self::Line(paint) => &paint.line_color,
            Self::Fill(paint) => &paint.fill_color,
            Self::Circle(paint) => &paint.circle_color,
            Self::FillExtrusion(paint) => &paint.fill_extrusion_color,
            Self::Raster(_)
            | Self::Hillshade(_)
            | Self::ColorRelief(_)
            | Self::Symbol(_)
            | Self::Heatmap(_) => return None,
        };
        match property.as_ref()? {
            StyleProperty::Constant(color) => Some(color.clone().into()),
            _ => None,
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

/// One entry in the style's painter order. Equality and hashing use only the layer ID.
/// Zoom bounds control display visibility independently of source tile zoom limits.
#[derive(Debug, Clone)]
pub struct StyleLayer {
    /// Zero-based painter order assigned when the containing style is deserialized.
    pub index: u32,
    /// Document-local identity used for equality, hashing and matching processed geometry.
    pub id: String,
    /// Rendering type from JSON, used to select paint parsing and the corresponding pipeline.
    pub type_: String,
    /// Feature-selection expression retained as JSON for filter validation and evaluation.
    pub filter: Option<serde_json::Value>,
    /// Exclusive upper display zoom; fractional values are preserved. `None` sets no upper bound.
    pub maxzoom: Option<f64>,
    /// Inclusive lower display zoom; fractional values are preserved. `None` sets no lower bound.
    pub minzoom: Option<f64>,
    /// Application JSON properties retained across worker serialization.
    /// Reserved `maplibre-rs:terrain-structure` keys opt into terrain-relative road profiles.
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    /// Typed rendering properties; absence lets the relevant rendering path choose defaults.
    pub paint: Option<LayerPaint>,
    /// Key of a source in the containing style, or `None` for source-independent or host-supplied data.
    pub source: Option<String>,
    /// Layer name inside a vector tile, distinct from the style layer ID.
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
            && self.minzoom.is_none_or(|minzoom| zoom >= minzoom)
            && self.maxzoom.is_none_or(|maxzoom| zoom < maxzoom)
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
