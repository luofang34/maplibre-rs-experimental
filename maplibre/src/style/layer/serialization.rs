//! Layer JSON keeps unrecognized properties so validation can identify their original paths.

use serde::{de::DeserializeOwned, ser::SerializeMap, Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{LayerPaint, LayerVisibility, StyleLayer, StyleProperty, SymbolPaint};

/// JSON properties grouped by their paint or layout scope.
#[derive(Debug, Clone, Default)]
pub struct LayerProperties {
    /// Paint entries, including their values.
    pub paint: Map<String, Value>,
    /// Layout entries, including their values.
    pub layout: Map<String, Value>,
}

#[derive(Deserialize)]
struct PaintWithExtras<T> {
    #[serde(flatten)]
    paint: T,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

fn parse_paint<T: DeserializeOwned>(
    value: Value,
    extra: &mut Map<String, Value>,
    wrap: impl FnOnce(T) -> LayerPaint,
) -> Result<LayerPaint, serde_json::Error> {
    let parsed: PaintWithExtras<T> = serde_json::from_value(value)?;
    *extra = parsed.extra;
    Ok(wrap(parsed.paint))
}

#[derive(Deserialize)]
struct StyleLayerDef {
    id: String,
    #[serde(rename = "type")]
    type_: String,
    filter: Option<Value>,
    maxzoom: Option<f64>,
    minzoom: Option<f64>,
    metadata: Option<std::collections::HashMap<String, Value>>,
    source: Option<String>,
    #[serde(rename = "source-layer")]
    source_layer: Option<String>,
    paint: Option<Value>,
    #[serde(default)]
    layout: Map<String, Value>,
}

impl StyleLayerDef {
    fn parse_paint(
        &mut self,
        extra: &mut Map<String, Value>,
    ) -> Result<Option<LayerPaint>, serde_json::Error> {
        let Some(paint) = self.paint.take() else {
            return Ok(match self.type_.as_str() {
                "circle" => Some(LayerPaint::Circle(Default::default())),
                "fill" => Some(LayerPaint::Fill(Default::default())),
                "line" => Some(LayerPaint::Line(Default::default())),
                "background" => Some(LayerPaint::Background(Default::default())),
                "fill-extrusion" => Some(LayerPaint::FillExtrusion(Default::default())),
                "heatmap" => Some(LayerPaint::Heatmap(Default::default())),
                "hillshade" => Some(LayerPaint::Hillshade(Default::default())),
                "color-relief" => Some(LayerPaint::ColorRelief(Default::default())),
                "symbol" => Some(LayerPaint::Symbol(Default::default())),
                _ => None,
            });
        };
        let parsed = match self.type_.as_str() {
            "background" => parse_paint(paint, extra, LayerPaint::Background)?,
            "line" => parse_paint(paint, extra, LayerPaint::Line)?,
            "fill" => parse_paint(paint, extra, LayerPaint::Fill)?,
            "fill-extrusion" => parse_paint(paint, extra, LayerPaint::FillExtrusion)?,
            "raster" => parse_paint(paint, extra, LayerPaint::Raster)?,
            "hillshade" => parse_paint(paint, extra, LayerPaint::Hillshade)?,
            "color-relief" => parse_paint(paint, extra, LayerPaint::ColorRelief)?,
            "circle" => parse_paint(paint, extra, LayerPaint::Circle)?,
            "heatmap" => parse_paint(paint, extra, LayerPaint::Heatmap)?,
            "symbol" => LayerPaint::Symbol(serde_json::from_value(paint)?),
            _ => {
                *extra = serde_json::from_value(paint)?;
                return Ok(None);
            }
        };
        Ok(Some(parsed))
    }
}

impl<'de> Deserialize<'de> for StyleLayer {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut def = StyleLayerDef::deserialize(deserializer)?;
        let mut unrecognized = LayerProperties::default();
        let mut paint = def.parse_paint(&mut unrecognized.paint).map_err(|error| {
            serde::de::Error::custom(format!("layer `{}` paint: {error}", def.id))
        })?;
        let visibility =
            def.layout
                .remove("visibility")
                .map_or(Ok(LayerVisibility::Visible), |value| {
                    serde_json::from_value(value).map_err(|error| {
                        serde::de::Error::custom(format!(
                            "layer `{}` layout.visibility: {error}",
                            def.id
                        ))
                    })
                })?;
        if let Some(LayerPaint::Symbol(symbol)) = &mut paint {
            merge_symbol_layout(symbol, def.layout, &mut unrecognized);
        } else {
            unrecognized.layout = def.layout;
        }
        Ok(Self {
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
            visibility,
            unrecognized,
        })
    }
}

fn merge_symbol_layout(
    symbol: &mut SymbolPaint,
    layout: Map<String, Value>,
    extra: &mut LayerProperties,
) {
    symbol.properties.retain(|name, value| {
        if crate::style::validation::symbol::supports(name) {
            true
        } else {
            extra.paint.insert(name.clone(), value.clone());
            false
        }
    });
    for (name, value) in layout {
        match name.as_str() {
            "text-field" => symbol.text_field = Some(StyleProperty::parse(&value)),
            "text-size" => symbol.text_size = Some(StyleProperty::parse(&value)),
            _ if crate::style::validation::symbol::supports(&name) => {
                symbol.properties.insert(name, value);
            }
            _ => {
                extra.layout.insert(name, value);
            }
        }
    }
}

impl Serialize for StyleLayer {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("id", &self.id)?;
        map.serialize_entry("type", &self.type_)?;
        if let Some(value) = &self.filter {
            map.serialize_entry("filter", value)?;
        }
        if let Some(value) = &self.maxzoom {
            map.serialize_entry("maxzoom", value)?;
        }
        if let Some(value) = &self.minzoom {
            map.serialize_entry("minzoom", value)?;
        }
        if let Some(value) = &self.metadata {
            map.serialize_entry("metadata", value)?;
        }
        if let Some(value) = &self.source {
            map.serialize_entry("source", value)?;
        }
        if let Some(value) = &self.source_layer {
            map.serialize_entry("source-layer", value)?;
        }
        let properties = self.property_maps().map_err(serde::ser::Error::custom)?;
        let LayerProperties { paint, layout } = properties;
        if !paint.is_empty() || self.paint.is_some() {
            map.serialize_entry("paint", &paint)?;
        }
        if !layout.is_empty() {
            map.serialize_entry("layout", &layout)?;
        }
        map.end()
    }
}

impl StyleLayer {
    fn property_maps(&self) -> Result<LayerProperties, serde_json::Error> {
        let mut paint = self.unrecognized.paint.clone();
        let mut layout = self.unrecognized.layout.clone();
        if let Some(model) = &self.paint {
            let tagged = serde_json::to_value(model)?;
            if let Some(properties) = tagged.get("paint").and_then(Value::as_object) {
                for (name, value) in properties {
                    let destination = if matches!(model, LayerPaint::Symbol(_))
                        && crate::style::validation::symbol::is_layout(name)
                    {
                        &mut layout
                    } else {
                        &mut paint
                    };
                    destination.insert(name.clone(), value.clone());
                }
            }
        }
        if self.is_hidden() {
            layout.insert("visibility".into(), Value::String("none".into()));
        }
        Ok(LayerProperties { paint, layout })
    }
}
