//! Vector tile layer drawing utilities.

use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
};

use cint::{Alpha, EncodedSrgb};
use csscolorparser::Color;
use serde::{Deserialize, Serialize};

use crate::style::{
    circle::CirclePaint,
    hillshade::{ColorReliefPaint, HillshadePaint},
};

pub use crate::style::property::{PropertyValue, StyleProperty, TextField};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BackgroundPaint {
    #[serde(rename = "background-color")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<Color>::deserialize_color_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background_color: Option<StyleProperty<Color>>,
    // TODO a lot
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

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct FillPaint {
    #[serde(rename = "fill-color")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<Color>::deserialize_color_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill_color: Option<StyleProperty<Color>>,
    /// Opacity multiplied into the fill colour, per feature where data driven.
    #[serde(rename = "fill-opacity")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<f32>::deserialize_f32_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill_opacity: Option<StyleProperty<f32>>,
    /// Translation in screen pixels before conversion to tile units.
    #[serde(rename = "fill-translate", default)]
    pub fill_translate: Option<[f32; 2]>,
    /// Coordinate frame for `fill_translate`.
    #[serde(rename = "fill-translate-anchor", default)]
    pub fill_translate_anchor: TranslateAnchor,
    // TODO a lot
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct LinePaint {
    #[serde(rename = "line-color")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<Color>::deserialize_color_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_color: Option<StyleProperty<Color>>,

    #[serde(rename = "line-width")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<f32>::deserialize_f32_or_none"
    )]
    pub line_width: Option<StyleProperty<f32>>,
    /// Opacity multiplied into the line colour, per feature where data driven.
    #[serde(rename = "line-opacity")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<f32>::deserialize_f32_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_opacity: Option<StyleProperty<f32>>,
    /// Translation in screen pixels before conversion to tile units.
    #[serde(rename = "line-translate", default)]
    pub line_translate: Option<[f32; 2]>,
    /// Coordinate frame for `line_translate`.
    #[serde(rename = "line-translate-anchor", default)]
    pub line_translate_anchor: TranslateAnchor,
    // TODO a lot
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum RasterResampling {
    #[serde(rename = "linear")]
    Linear,
    #[serde(rename = "nearest")]
    Nearest,
}

/// Raster tile layer description
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RasterPaint {
    #[serde(rename = "raster-brightness-max")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raster_brightness_max: Option<f32>,
    #[serde(rename = "raster-brightness-min")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raster_brightness_min: Option<f32>,
    #[serde(rename = "raster-contrast")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raster_contrast: Option<f32>,
    #[serde(rename = "raster-fade-duration")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raster_fade_duration: Option<u32>,
    #[serde(rename = "raster-hue-rotate")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raster_hue_rotate: Option<f32>,
    #[serde(rename = "raster-opacity")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raster_opacity: Option<f32>,
    #[serde(rename = "raster-resampling")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raster_resampling: Option<RasterResampling>,
    #[serde(rename = "raster-saturation")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raster_saturation: Option<f32>,
}

impl Default for RasterPaint {
    fn default() -> Self {
        RasterPaint {
            raster_brightness_max: Some(1.0),
            raster_brightness_min: Some(0.0),
            raster_contrast: Some(0.0),
            raster_fade_duration: Some(0),
            raster_hue_rotate: Some(0.0),
            raster_opacity: Some(1.0),
            raster_resampling: Some(RasterResampling::Linear),
            raster_saturation: Some(0.0),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SymbolPaint {
    /// Text of each symbol: a `{token}` template, a literal, or an expression; `None` draws
    /// no text.
    #[serde(rename = "text-field")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<TextField>::deserialize_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_field: Option<StyleProperty<TextField>>,

    #[serde(rename = "text-size")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<f32>::deserialize_f32_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_size: Option<StyleProperty<f32>>,
    // TODO a lot
}

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
mod tests {
    use crate::style::expression::{FeatureProperties, Value};

    #[test]
    fn zoom_range_is_min_inclusive_max_exclusive() {
        let mut layer = super::StyleLayer {
            index: 0,
            id: "labels".to_string(),
            type_: "symbol".to_string(),
            filter: None,
            maxzoom: Some(6),
            minzoom: Some(2),
            metadata: None,
            paint: None,
            source: None,
            source_layer: None,
            visibility: super::LayerVisibility::Visible,
        };

        assert!(!layer.is_visible_at(1.99));
        assert!(layer.is_visible_at(2.0));
        assert!(layer.is_visible_at(5.99));
        assert!(!layer.is_visible_at(6.0));

        layer.minzoom = None;
        layer.maxzoom = None;
        assert!(layer.is_visible_at(0.0));
        assert!(layer.is_visible_at(24.0));
    }

    #[test]
    fn a_layer_with_visibility_none_is_hidden_at_every_zoom() {
        let hidden: super::StyleLayer = serde_json::from_value(serde_json::json!({
            "id": "water", "type": "fill", "source": "s", "source-layer": "water",
            "layout": {"visibility": "none"}
        }))
        .expect("layer parses");
        assert!(hidden.is_hidden());
        assert!(!hidden.is_visible_at(0.0) && !hidden.is_visible_at(12.0));

        for layout in [
            serde_json::json!({"visibility": "visible"}),
            serde_json::json!({}),
        ] {
            let visible: super::StyleLayer = serde_json::from_value(serde_json::json!({
                "id": "water", "type": "fill", "source": "s", "source-layer": "water",
                "layout": layout
            }))
            .expect("layer parses");
            assert!(!visible.is_hidden());
            assert!(visible.is_visible_at(0.0));
        }
    }

    use super::*;

    #[test]
    fn test_evaluate_match_missing_property_returns_fallback() {
        let json = r#"
        [
            "match",
            ["get", "ADM0_A3"],
            ["ARM", "ATG"],
            "rgba(1, 2, 3, 1)",
            "rgba(9, 9, 9, 1)"
        ]
        "#;
        let expr: serde_json::Value = serde_json::from_str(json).unwrap();
        let prop: StyleProperty<csscolorparser::Color> = StyleProperty::parse(&expr);

        // Feature that does NOT have the property → should return the JSON fallback color
        let empty_props = FeatureProperties::new();
        let color = prop.evaluate_for(&empty_props, 0.0).unwrap();
        assert_eq!(color.to_rgba8(), [9, 9, 9, 255]);
    }

    #[test]
    fn fill_and_line_layers_carry_their_opacity_property() {
        let fill: StyleLayer = serde_json::from_value(serde_json::json!({
            "id": "water", "type": "fill", "source": "s",
            "paint": {"fill-color": "#0000ff", "fill-opacity": 0.3}
        }))
        .expect("layer parses");
        let line: StyleLayer = serde_json::from_value(serde_json::json!({
            "id": "road", "type": "line", "source": "s",
            "paint": {"line-opacity": {"stops": [[0, 0.5], [1, 0.6]]}}
        }))
        .expect("layer parses");
        let plain: StyleLayer = serde_json::from_value(serde_json::json!({
            "id": "land", "type": "fill", "source": "s", "paint": {"fill-color": "#00ff00"}
        }))
        .expect("layer parses");

        let opacity = |layer: &StyleLayer| layer.paint.as_ref().and_then(LayerPaint::opacity);
        assert!(matches!(opacity(&fill), Some(StyleProperty::Constant(value)) if value == 0.3));
        assert!(
            opacity(&line)
                .expect("line opacity")
                .evaluate_at_zoom(0.5)
                .is_some_and(|value| (value - 0.55).abs() < 1e-6),
            "zoom functions evaluate at the zoom"
        );
        assert!(opacity(&plain).is_none());
    }

    #[test]
    fn test_evaluate_match() {
        let json = r#"
        [
            "match",
            ["get", "ADM0_A3"],
            ["ARM", "ATG"],
            "rgba(1, 2, 3, 1)",
            "rgba(0, 0, 0, 1)"
        ]
        "#;
        let expr: serde_json::Value = serde_json::from_str(json).unwrap();
        let prop: StyleProperty<csscolorparser::Color> = StyleProperty::parse(&expr);

        let mut feature_properties = FeatureProperties::new();
        feature_properties.insert("ADM0_A3".to_string(), Value::from("ARM"));

        let color = prop.evaluate_for(&feature_properties, 0.0).unwrap();
        assert_eq!(color.to_rgba8(), [1, 2, 3, 255]);
    }

    #[test]
    fn test_symbol_text_field_from_layout() {
        let json = r#"{
            "id": "countries-label",
            "type": "symbol",
            "paint": {
                "text-color": "rgba(8, 37, 77, 1)"
            },
            "layout": {
                "text-field": "{NAME}",
                "text-font": ["Open Sans Semibold"]
            },
            "source": "maplibre",
            "source-layer": "centroids"
        }"#;
        let layer: StyleLayer = serde_json::from_str(json).unwrap();
        assert_eq!(layer.type_, "symbol");
        match &layer.paint {
            Some(LayerPaint::Symbol(sp)) => {
                assert_eq!(text_of(sp, 3.0).as_deref(), Some("Berlin"));
            }
            other => panic!("expected Symbol paint, got {:?}", other),
        }
    }

    /// The text a symbol paint produces for a feature named `Berlin` and abbreviated `BER`.
    fn text_of(paint: &SymbolPaint, zoom: f64) -> Option<String> {
        let properties = crate::style::expression::FeatureProperties::from([
            ("NAME".to_string(), Value::String("Berlin".to_string())),
            ("ABBREV".to_string(), Value::String("BER".to_string())),
        ]);
        paint
            .text_field
            .as_ref()?
            .evaluate_for(&properties, zoom)
            .map(|text| text.0)
    }

    #[test]
    fn test_symbol_text_field_zoom_dependent() {
        let json = r#"{
            "id": "test-label",
            "type": "symbol",
            "paint": {},
            "layout": {
                "text-field": {"stops": [[2, "{ABBREV}"], [4, "{NAME}"]]}
            },
            "source": "maplibre",
            "source-layer": "centroids"
        }"#;
        let layer: StyleLayer = serde_json::from_str(json).unwrap();
        match &layer.paint {
            Some(LayerPaint::Symbol(sp)) => {
                assert_eq!(text_of(sp, 3.0).as_deref(), Some("BER"));
                assert_eq!(text_of(sp, 5.0).as_deref(), Some("Berlin"));
            }
            other => panic!("expected Symbol paint, got {:?}", other),
        }
    }

    #[test]
    fn test_demotiles_symbol_layers_have_text_field() {
        let style: crate::style::Style = Default::default();
        for layer in &style.layers {
            if layer.type_ == "symbol" {
                match &layer.paint {
                    Some(LayerPaint::Symbol(sp)) => {
                        assert!(
                            sp.text_field.is_some(),
                            "symbol layer '{}' should have text_field parsed from layout",
                            layer.id
                        );
                    }
                    _ => panic!("symbol layer '{}' has no Symbol paint", layer.id),
                }
            }
        }
    }

    #[test]
    fn parses_fill_and_line_translation_properties() {
        let style: crate::style::Style = serde_json::from_str(
            r#"{
                "version": 8,
                "sources": {},
                "layers": [
                    {
                        "id": "fill",
                        "type": "fill",
                        "paint": {
                            "fill-color": "red",
                            "fill-translate": [10, 50],
                            "fill-translate-anchor": "viewport"
                        }
                    },
                    {
                        "id": "line",
                        "type": "line",
                        "paint": {
                            "line-color": "blue",
                            "line-translate": [2, 3]
                        }
                    }
                ]
            }"#,
        )
        .unwrap();

        let Some(LayerPaint::Fill(fill)) = style.layers[0].paint.as_ref() else {
            panic!("first layer should be a fill");
        };
        assert_eq!(fill.fill_translate, Some([10.0, 50.0]));
        assert_eq!(fill.fill_translate_anchor, TranslateAnchor::Viewport);

        let Some(LayerPaint::Line(line)) = style.layers[1].paint.as_ref() else {
            panic!("second layer should be a line");
        };
        assert_eq!(line.line_translate, Some([2.0, 3.0]));
        assert_eq!(line.line_translate_anchor, TranslateAnchor::Map);
    }
}
