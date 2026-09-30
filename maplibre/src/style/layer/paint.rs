//! Typed paint fields and symbol layout values retained by layer serialization.

use csscolorparser::Color;
use serde::{Deserialize, Serialize};

use super::{StyleProperty, TextField};

/// Base background color; properties outside this model are retained by layer serialization.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BackgroundPaint {
    /// Background color property; rendering supports constant colors.
    #[serde(rename = "background-color")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<Color>::deserialize_color_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background_color: Option<StyleProperty<Color>>,
    /// Opacity multiplied into the background color.
    #[serde(rename = "background-opacity")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<f32>::deserialize_f32_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background_opacity: Option<StyleProperty<f32>>,
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

/// Polygon appearance; absent fields defer to the rendering path's defaults.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct FillPaint {
    /// Per-feature fill color property, before multiplying layer opacity.
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
    /// The style's `fill-antialias`, kept as written so validation can tell a request the
    /// renderer honours from one it cannot.
    #[serde(rename = "fill-antialias", default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill_antialias: Option<serde_json::Value>,
    /// Translation in screen pixels before conversion to tile units.
    #[serde(rename = "fill-translate", default)]
    pub fill_translate: Option<[f32; 2]>,
    /// Coordinate frame for `fill_translate`.
    #[serde(rename = "fill-translate-anchor", default)]
    pub fill_translate_anchor: TranslateAnchor,
}

/// Path appearance with widths and translations expressed in screen pixels.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct LinePaint {
    /// Per-feature stroke color property, before multiplying layer opacity.
    #[serde(rename = "line-color")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<Color>::deserialize_color_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_color: Option<StyleProperty<Color>>,

    /// Stroke width in screen pixels, evaluated at the current zoom.
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
}

/// Requested raster texture filter; validation reports unsupported sampling modes.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum RasterResampling {
    /// Interpolates neighboring texels.
    #[serde(rename = "linear")]
    Linear,
    /// Selects the nearest texel without interpolation.
    #[serde(rename = "nearest")]
    Nearest,
}

/// Raster image adjustments retained for serialization and validation.
/// Rendering supports only neutral adjustments and linear sampling; [`crate::style::Style::validate`]
/// reports non-neutral values. `Default` sets neutral values and disables fading.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RasterPaint {
    /// Upper brightness mapping bound in 0..=1; the neutral value is 1.
    #[serde(rename = "raster-brightness-max")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raster_brightness_max: Option<f32>,
    /// Lower brightness mapping bound in 0..=1; the neutral value is 0.
    #[serde(rename = "raster-brightness-min")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raster_brightness_min: Option<f32>,
    /// Contrast adjustment in -1..=1; zero preserves the source contrast.
    #[serde(rename = "raster-contrast")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raster_contrast: Option<f32>,
    /// Requested tile fade duration in milliseconds; zero disables fading.
    #[serde(rename = "raster-fade-duration")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raster_fade_duration: Option<u32>,
    /// Requested hue rotation in degrees; zero preserves source colors.
    #[serde(rename = "raster-hue-rotate")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raster_hue_rotate: Option<f32>,
    /// Layer alpha multiplier in 0..=1; one preserves source alpha.
    #[serde(rename = "raster-opacity")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raster_opacity: Option<f32>,
    /// Requested texture filter; omission and the renderer default use linear sampling.
    #[serde(rename = "raster-resampling")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raster_resampling: Option<RasterResampling>,
    /// Saturation adjustment in -1..=1; zero preserves source saturation.
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

/// Typed text properties and additional text/icon paint and layout values.
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

    /// Text em size in screen pixels, evaluated for each feature and zoom.
    #[serde(rename = "text-size")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<f32>::deserialize_f32_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_size: Option<StyleProperty<f32>>,
    /// Symbol paint and layout properties evaluated during placement.
    #[serde(flatten)]
    pub properties: serde_json::Map<String, serde_json::Value>,
}
