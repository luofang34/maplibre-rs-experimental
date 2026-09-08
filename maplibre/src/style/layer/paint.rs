use super::{StyleProperty, TextField};
use csscolorparser::Color;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BackgroundPaint {
    #[serde(rename = "background-color")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<Color>::deserialize_color_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background_color: Option<StyleProperty<Color>>,
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
    /// Alternating dash and gap lengths in line-width units, evaluated at integer zoom.
    #[serde(
        rename = "line-dasharray",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub line_dasharray: Option<serde_json::Value>,
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

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
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
    /// Symbol paint and layout properties evaluated during placement.
    #[serde(flatten)]
    pub properties: serde_json::Map<String, serde_json::Value>,
}
