//! Typed paint fields and symbol layout values retained by layer serialization.

use csscolorparser::Color;
use serde::{Deserialize, Serialize};

use super::{StyleProperty, TextField};

/// Base background color; properties outside this model are retained by layer serialization.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct BackgroundPaint {
    /// Background color property; rendering supports constant colors.
    #[serde(rename = "background-color")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<Color>::deserialize_color_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background_color: Option<StyleProperty<Color>>,
    /// Name of the image the background repeats instead of a colour, as written.
    #[serde(rename = "background-pattern", default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background_pattern: Option<serde_json::Value>,
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
    /// Name of the image the polygon repeats instead of a colour, as written: a string, a
    /// zoom expression or a feature expression.
    #[serde(rename = "fill-pattern", default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill_pattern: Option<serde_json::Value>,
    /// The style's `fill-antialias`, kept as written so the style serializes unchanged.
    #[serde(rename = "fill-antialias", default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill_antialias: Option<serde_json::Value>,
    /// Translation in screen pixels before conversion to tile units.
    #[serde(rename = "fill-translate", default)]
    pub fill_translate: Option<crate::style::translation::Translation>,
    /// Coordinate frame for `fill_translate`.
    #[serde(rename = "fill-translate-anchor", default)]
    pub fill_translate_anchor: TranslateAnchor,
}

/// Extruded polygon appearance; heights are in metres above the ground.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct FillExtrusionPaint {
    /// Colour of the walls and roof, per feature where data driven.
    #[serde(rename = "fill-extrusion-color")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<Color>::deserialize_color_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill_extrusion_color: Option<StyleProperty<Color>>,
    /// Name of the image the walls and roof repeat instead of a colour, as written.
    #[serde(rename = "fill-extrusion-pattern", default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill_extrusion_pattern: Option<serde_json::Value>,
    /// Opacity of the whole layer at the view zoom.
    #[serde(rename = "fill-extrusion-opacity")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<f32>::deserialize_f32_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill_extrusion_opacity: Option<StyleProperty<f32>>,
    /// Height of the roof above the ground, per feature where data driven.
    #[serde(rename = "fill-extrusion-height")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<f32>::deserialize_f32_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill_extrusion_height: Option<StyleProperty<f32>>,
    /// Height of the bottom of the walls above the ground, per feature where data driven.
    #[serde(rename = "fill-extrusion-base")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<f32>::deserialize_f32_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill_extrusion_base: Option<StyleProperty<f32>>,
    /// Whether the walls darken towards their base; defaults to on.
    #[serde(rename = "fill-extrusion-vertical-gradient", default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill_extrusion_vertical_gradient: Option<bool>,
    /// Translation in screen pixels before conversion to tile units.
    #[serde(rename = "fill-extrusion-translate", default)]
    pub fill_extrusion_translate: Option<crate::style::translation::Translation>,
    /// Coordinate frame for `fill_extrusion_translate`.
    #[serde(rename = "fill-extrusion-translate-anchor", default)]
    pub fill_extrusion_translate_anchor: TranslateAnchor,
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
    pub line_translate: Option<crate::style::translation::Translation>,
    /// Coordinate frame for `line_translate`.
    #[serde(rename = "line-translate-anchor", default)]
    pub line_translate_anchor: TranslateAnchor,
    /// Colour along the line as a function of `["line-progress"]`; replaces `line-color`.
    #[serde(
        rename = "line-gradient",
        default,
        deserialize_with = "StyleProperty::<Color>::deserialize_color_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_gradient: Option<StyleProperty<Color>>,
    /// Distance in pixels the line is moved to the right of its direction, at the view zoom.
    #[serde(
        rename = "line-offset",
        default,
        deserialize_with = "StyleProperty::<f32>::deserialize_f32_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_offset: Option<StyleProperty<f32>>,
    /// Width in pixels of the empty band along the middle of the line, at the view zoom.
    #[serde(
        rename = "line-gap-width",
        default,
        deserialize_with = "StyleProperty::<f32>::deserialize_f32_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_gap_width: Option<StyleProperty<f32>>,
    /// Blur of the line's edges in pixels, at the view zoom.
    #[serde(
        rename = "line-blur",
        default,
        deserialize_with = "StyleProperty::<f32>::deserialize_f32_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_blur: Option<StyleProperty<f32>>,
    /// Name of the image drawn along the line instead of a colour, as written: a string, a
    /// zoom expression or a feature expression.
    #[serde(rename = "line-pattern", default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_pattern: Option<serde_json::Value>,
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
    #[serde(
        default,
        deserialize_with = "StyleProperty::<f32>::deserialize_f32_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raster_brightness_max: Option<StyleProperty<f32>>,
    /// Lower brightness mapping bound in 0..=1; the neutral value is 0.
    #[serde(rename = "raster-brightness-min")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<f32>::deserialize_f32_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raster_brightness_min: Option<StyleProperty<f32>>,
    /// Contrast adjustment in -1..=1; zero preserves the source contrast.
    #[serde(rename = "raster-contrast")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<f32>::deserialize_f32_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raster_contrast: Option<StyleProperty<f32>>,
    /// Requested tile fade duration in milliseconds; zero disables fading.
    #[serde(rename = "raster-fade-duration")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raster_fade_duration: Option<u32>,
    /// Requested hue rotation in degrees; zero preserves source colors.
    #[serde(rename = "raster-hue-rotate")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<f32>::deserialize_f32_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raster_hue_rotate: Option<StyleProperty<f32>>,
    /// Layer alpha multiplier in 0..=1; one preserves source alpha.
    #[serde(rename = "raster-opacity")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<f32>::deserialize_f32_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raster_opacity: Option<StyleProperty<f32>>,
    /// Requested texture filter; omission and the renderer default use linear sampling.
    #[serde(rename = "raster-resampling")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raster_resampling: Option<RasterResampling>,
    /// Saturation adjustment in -1..=1; zero preserves source saturation.
    #[serde(rename = "raster-saturation")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<f32>::deserialize_f32_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raster_saturation: Option<StyleProperty<f32>>,
}

impl Default for RasterPaint {
    fn default() -> Self {
        RasterPaint {
            raster_brightness_max: Some(StyleProperty::Constant(1.0)),
            raster_brightness_min: Some(StyleProperty::Constant(0.0)),
            raster_contrast: Some(StyleProperty::Constant(0.0)),
            raster_fade_duration: Some(0),
            raster_hue_rotate: Some(StyleProperty::Constant(0.0)),
            raster_opacity: Some(StyleProperty::Constant(1.0)),
            raster_resampling: Some(RasterResampling::Linear),
            raster_saturation: Some(StyleProperty::Constant(0.0)),
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
