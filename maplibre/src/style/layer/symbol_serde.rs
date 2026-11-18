//! Manual serde and known-key definitions for style layers to avoid `#[serde(flatten)]` bloat.

use std::fmt;

use serde::{
    de::{self, DeserializeSeed, MapAccess, Visitor},
    ser::SerializeMap,
    Deserialize, Serialize,
};
use serde_json::{Map, Value};

use super::{
    paint::{
        BackgroundPaint, CustomPaint, FillExtrusionPaint, FillPaint, LinePaint, RasterPaint,
        SymbolPaint,
    },
    FormattedText, StyleProperty,
};
use crate::style::{
    circle::CirclePaint,
    heatmap::HeatmapPaint,
    hillshade::{ColorReliefPaint, HillshadePaint},
};

/// Known typed paint property keys for splitting unrecognized keys without `#[serde(flatten)]`.
pub(super) trait KnownPaintKeys {
    const KNOWN_KEYS: &'static [&'static str];
}

impl KnownPaintKeys for BackgroundPaint {
    const KNOWN_KEYS: &'static [&'static str] = &[
        "background-color",
        "background-pattern",
        "background-opacity",
    ];
}

impl KnownPaintKeys for FillPaint {
    const KNOWN_KEYS: &'static [&'static str] = &[
        "fill-color",
        "fill-outline-color",
        "fill-opacity",
        "fill-pattern",
        "fill-antialias",
        "fill-translate",
        "fill-translate-anchor",
    ];
}

impl KnownPaintKeys for FillExtrusionPaint {
    const KNOWN_KEYS: &'static [&'static str] = &[
        "fill-extrusion-color",
        "fill-extrusion-pattern",
        "fill-extrusion-opacity",
        "fill-extrusion-height",
        "fill-extrusion-base",
        "fill-extrusion-vertical-gradient",
        "fill-extrusion-translucent",
        "fill-extrusion-base-datum",
        "fill-extrusion-height-datum",
        "fill-extrusion-part",
        "fill-extrusion-rounded-corner-distance",
        "fill-extrusion-translate",
        "fill-extrusion-translate-anchor",
    ];
}

impl KnownPaintKeys for LinePaint {
    const KNOWN_KEYS: &'static [&'static str] = &[
        "line-color",
        "line-width",
        "line-opacity",
        "line-translate",
        "line-translate-anchor",
        "line-gradient",
        "line-offset",
        "line-gap-width",
        "line-blur",
        "line-pattern",
        "line-dasharray",
        "line-elevation",
        "line-elevation-datum",
    ];
}

impl KnownPaintKeys for RasterPaint {
    const KNOWN_KEYS: &'static [&'static str] = &[
        "raster-brightness-max",
        "raster-brightness-min",
        "raster-contrast",
        "raster-fade-duration",
        "raster-hue-rotate",
        "raster-opacity",
        "raster-resampling",
        "raster-saturation",
    ];
}

impl KnownPaintKeys for HillshadePaint {
    const KNOWN_KEYS: &'static [&'static str] = &[
        "hillshade-illumination-direction",
        "hillshade-illumination-altitude",
        "hillshade-illumination-anchor",
        "hillshade-exaggeration",
        "hillshade-shadow-color",
        "hillshade-highlight-color",
        "hillshade-accent-color",
        "hillshade-method",
        "hillshade-resampling",
    ];
}

impl KnownPaintKeys for ColorReliefPaint {
    const KNOWN_KEYS: &'static [&'static str] = &[
        "color-relief-opacity",
        "color-relief-color",
        "color-relief-resampling",
    ];
}

impl KnownPaintKeys for CirclePaint {
    const KNOWN_KEYS: &'static [&'static str] = &[
        "circle-radius",
        "circle-color",
        "circle-blur",
        "circle-opacity",
        "circle-stroke-width",
        "circle-stroke-color",
        "circle-stroke-opacity",
        "circle-translate",
        "circle-translate-anchor",
        "circle-pitch-scale",
        "circle-pitch-alignment",
    ];
}

impl KnownPaintKeys for HeatmapPaint {
    const KNOWN_KEYS: &'static [&'static str] = &[
        "heatmap-radius",
        "heatmap-weight",
        "heatmap-intensity",
        "heatmap-opacity",
        "heatmap-color",
    ];
}

impl KnownPaintKeys for CustomPaint {
    const KNOWN_KEYS: &'static [&'static str] = &[];
}

struct OptTextFieldSeed;

impl<'de> DeserializeSeed<'de> for OptTextFieldSeed {
    type Value = Option<StyleProperty<FormattedText>>;

    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        StyleProperty::<FormattedText>::deserialize_or_none(deserializer)
    }
}

struct OptF32Seed;

impl<'de> DeserializeSeed<'de> for OptF32Seed {
    type Value = Option<StyleProperty<f32>>;

    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        StyleProperty::<f32>::deserialize_f32_or_none(deserializer)
    }
}

impl Serialize for SymbolPaint {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        if let Some(field) = &self.text_field {
            map.serialize_entry("text-field", field)?;
        }
        if let Some(size) = &self.text_size {
            map.serialize_entry("text-size", size)?;
        }
        for (key, value) in &self.properties {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for SymbolPaint {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SymbolPaintVisitor;

        impl<'de> Visitor<'de> for SymbolPaintVisitor {
            type Value = SymbolPaint;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a symbol paint map")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut access: M) -> Result<SymbolPaint, M::Error> {
                let mut text_field = None;
                let mut text_size = None;
                let mut properties = Map::new();
                while let Some(key) = access.next_key::<String>()? {
                    match key.as_str() {
                        "text-field" => {
                            text_field = access.next_value_seed(OptTextFieldSeed)?;
                        }
                        "text-size" => {
                            text_size = access.next_value_seed(OptF32Seed)?;
                        }
                        _ => {
                            let val: Value = access.next_value()?;
                            properties.insert(key, val);
                        }
                    }
                }
                Ok(SymbolPaint {
                    text_field,
                    text_size,
                    properties,
                    compiled: Default::default(),
                })
            }
        }

        deserializer.deserialize_map(SymbolPaintVisitor)
    }
}
