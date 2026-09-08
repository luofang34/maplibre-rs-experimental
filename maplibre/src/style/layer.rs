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
pub use paint::{
    BackgroundPaint, FillPaint, LinePaint, RasterPaint, RasterResampling, SymbolPaint,
    TranslateAnchor,
};

/// The `text-field` of a layout: a `{token}` template, a literal, a legacy function or an
/// expression.
fn parse_text_field_from_layout(layout: &serde_json::Value) -> Option<StyleProperty<TextField>> {
    Some(StyleProperty::parse(layout.get("text-field")?))
}

/// The `text-size` of a layout: a constant, a legacy function or an expression.
fn parse_text_size_from_layout(layout: &serde_json::Value) -> Option<StyleProperty<f32>> {
    Some(StyleProperty::parse(layout.get("text-size")?))
}

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

impl Serialize for StyleLayer {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        // Count non-None optional fields
        let mut count = 2; // id + type are always present
        if self.filter.is_some() {
            count += 1;
        }
        if self.maxzoom.is_some() {
            count += 1;
        }
        if self.minzoom.is_some() {
            count += 1;
        }
        if self.metadata.is_some() {
            count += 1;
        }
        if self.paint.is_some() {
            count += 1;
        }
        if self.source.is_some() {
            count += 1;
        }
        if self.source_layer.is_some() {
            count += 1;
        }
        let mut map = serializer.serialize_map(Some(count))?;
        map.serialize_entry("id", &self.id)?;
        map.serialize_entry("type", &self.type_)?;
        if let Some(ref filter) = self.filter {
            map.serialize_entry("filter", filter)?;
        }
        if let Some(ref maxzoom) = self.maxzoom {
            map.serialize_entry("maxzoom", maxzoom)?;
        }
        if let Some(ref minzoom) = self.minzoom {
            map.serialize_entry("minzoom", minzoom)?;
        }
        if let Some(ref metadata) = self.metadata {
            map.serialize_entry("metadata", metadata)?;
        }
        if let Some(ref paint) = self.paint {
            // Serialize just the inner paint data (without the LayerPaint tag)
            match paint {
                LayerPaint::Background(p) => map.serialize_entry("paint", p)?,
                LayerPaint::Line(p) => map.serialize_entry("paint", p)?,
                LayerPaint::Fill(p) => map.serialize_entry("paint", p)?,
                LayerPaint::Raster(p) => map.serialize_entry("paint", p)?,
                LayerPaint::Hillshade(p) => map.serialize_entry("paint", p)?,
                LayerPaint::ColorRelief(p) => map.serialize_entry("paint", p)?,
                LayerPaint::Symbol(p) => map.serialize_entry("paint", p)?,
                LayerPaint::Circle(p) => map.serialize_entry("paint", p)?,
            }
        }
        if let Some(ref source) = self.source {
            map.serialize_entry("source", source)?;
        }
        if let Some(ref source_layer) = self.source_layer {
            map.serialize_entry("source-layer", source_layer)?;
        }
        map.end()
    }
}

#[derive(Deserialize)]
struct StyleLayerDef {
    id: String,
    #[serde(rename = "type")]
    type_: String,
    filter: Option<serde_json::Value>,
    maxzoom: Option<u8>,
    minzoom: Option<u8>,
    metadata: Option<HashMap<String, String>>,
    source: Option<String>,
    #[serde(rename = "source-layer")]
    source_layer: Option<String>,
    paint: Option<serde_json::Value>,
    layout: Option<serde_json::Value>,
}

impl<'de> serde::Deserialize<'de> for StyleLayer {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let def = StyleLayerDef::deserialize(deserializer)?;

        let paint = if let Some(p) = def.paint {
            match def.type_.as_str() {
                "background" => serde_json::from_value(p.clone())
                    .map(LayerPaint::Background)
                    .ok(),
                "line" => serde_json::from_value(p.clone())
                    .map(LayerPaint::Line)
                    .map_err(|e| log::error!("line paint failed {}: {:?}", def.id, e))
                    .ok(),
                "fill" => serde_json::from_value(p.clone())
                    .map(LayerPaint::Fill)
                    .map_err(|e| log::error!("fill paint failed {}: {:?}", def.id, e))
                    .ok(),
                "raster" => serde_json::from_value(p.clone())
                    .map(LayerPaint::Raster)
                    .ok(),
                "hillshade" => serde_json::from_value(p.clone())
                    .map(LayerPaint::Hillshade)
                    .map_err(|e| log::error!("hillshade paint failed {}: {:?}", def.id, e))
                    .ok(),
                "color-relief" => serde_json::from_value(p.clone())
                    .map(LayerPaint::ColorRelief)
                    .map_err(|e| log::error!("color-relief paint failed {}: {:?}", def.id, e))
                    .ok(),
                "circle" => serde_json::from_value(p.clone())
                    .map(LayerPaint::Circle)
                    .map_err(|e| log::error!("circle paint failed {}: {:?}", def.id, e))
                    .ok(),
                "symbol" => {
                    let mut paint: Option<SymbolPaint> = serde_json::from_value(p.clone())
                        .map_err(|e| log::error!("symbol paint failed {}: {:?}", def.id, e))
                        .ok();
                    // text-field and text-size live in layout, not paint — merge them in
                    if let (Some(sp), Some(layout)) = (paint.as_mut(), def.layout.as_ref()) {
                        if let Some(properties) = layout.as_object() {
                            sp.properties.extend(properties.clone());
                        }
                        if sp.text_field.is_none() {
                            sp.text_field = parse_text_field_from_layout(layout);
                        }
                        if sp.text_size.is_none() {
                            sp.text_size = parse_text_size_from_layout(layout);
                        }
                    }
                    paint.map(LayerPaint::Symbol)
                }
                _ => None,
            }
        } else if def.type_ == "circle" {
            // Every circle paint property has a specification default, so a layer without
            // paint still draws.
            Some(LayerPaint::Circle(CirclePaint::default()))
        } else if def.type_ == "hillshade" {
            Some(LayerPaint::Hillshade(HillshadePaint::default()))
        } else if def.type_ == "color-relief" {
            Some(LayerPaint::ColorRelief(ColorReliefPaint::default()))
        } else if def.type_ == "symbol" {
            // Symbol layers may have no paint but still have layout with text-field/text-size
            let text_field = def.layout.as_ref().and_then(parse_text_field_from_layout);
            let text_size = def.layout.as_ref().and_then(parse_text_size_from_layout);
            Some(LayerPaint::Symbol(SymbolPaint {
                text_field,
                text_size,
                properties: def
                    .layout
                    .as_ref()
                    .and_then(|layout| layout.as_object())
                    .cloned()
                    .unwrap_or_default(),
            }))
        } else {
            None
        };

        Ok(StyleLayer {
            index: 0,
            id: def.id,
            type_: def.type_,
            filter: def.filter,
            maxzoom: def.maxzoom,
            minzoom: def.minzoom,
            metadata: def.metadata,
            paint,
            source: def.source,
            source_layer: def.source_layer,
            visibility: def
                .layout
                .as_ref()
                .and_then(|layout| layout.get("visibility"))
                .and_then(|visibility| serde_json::from_value(visibility.clone()).ok())
                .unwrap_or_default(),
        })
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
        }
    }
}

#[cfg(test)]
mod tests;
