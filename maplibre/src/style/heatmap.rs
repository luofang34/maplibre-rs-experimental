//! Paint properties of `heatmap` layers, following the GL JS style specification defaults.

use csscolorparser::Color;
use serde::{Deserialize, Serialize};

use crate::style::{
    expression::{Color as ExpressionColor, EvaluationContext, Value},
    layer::StyleProperty,
};

/// Texels in the colour ramp a heatmap samples its density through.
pub const RAMP_TEXELS: usize = 256;

/// The specification's default `heatmap-color`.
const DEFAULT_COLOR: &str = r#"["interpolate", ["linear"], ["heatmap-density"],
    0, "rgba(0, 0, 255, 0)", 0.1, "royalblue", 0.3, "cyan", 0.5, "lime", 0.7, "yellow", 1, "red"]"#;

/// Paint of a `heatmap` layer; an absent property means the specification default.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct HeatmapPaint {
    /// Radius of influence of one point in screen pixels; 30 by default.
    #[serde(rename = "heatmap-radius")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<f32>::deserialize_f32_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heatmap_radius: Option<StyleProperty<f32>>,
    /// How much one point contributes to the density; 1 by default.
    #[serde(rename = "heatmap-weight")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<f32>::deserialize_f32_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heatmap_weight: Option<StyleProperty<f32>>,
    /// Multiplier on the density of every point; 1 by default.
    #[serde(rename = "heatmap-intensity")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<f32>::deserialize_f32_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heatmap_intensity: Option<StyleProperty<f32>>,
    /// Opacity of the whole layer; 1 by default.
    #[serde(rename = "heatmap-opacity")]
    #[serde(
        default,
        deserialize_with = "StyleProperty::<f32>::deserialize_f32_or_none"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heatmap_opacity: Option<StyleProperty<f32>>,
    /// Colour as a function of `["heatmap-density"]`.
    #[serde(rename = "heatmap-color", default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heatmap_color: Option<StyleProperty<Color>>,
}

impl HeatmapPaint {
    /// Radius in pixels at `zoom`.
    pub fn radius_at(&self, zoom: f64) -> f32 {
        Self::number(&self.heatmap_radius, zoom, 30.0)
    }

    /// Intensity at `zoom`.
    pub fn intensity_at(&self, zoom: f64) -> f32 {
        Self::number(&self.heatmap_intensity, zoom, 1.0)
    }

    /// Opacity at `zoom`.
    pub fn opacity_at(&self, zoom: f64) -> f32 {
        Self::number(&self.heatmap_opacity, zoom, 1.0).clamp(0.0, 1.0)
    }

    fn number(property: &Option<StyleProperty<f32>>, zoom: f64, default: f32) -> f32 {
        property
            .as_ref()
            .and_then(|property| property.evaluate_at_zoom(zoom))
            .unwrap_or(default)
    }

    /// The colour of each of [`RAMP_TEXELS`] densities from 0 to 1, premultiplied 8-bit RGBA.
    ///
    /// An expression that cannot be evaluated falls back to the default ramp, so a heatmap
    /// never draws with an undefined colour.
    pub fn ramp(&self) -> Vec<[u8; 4]> {
        let colour_at = |property: &StyleProperty<Color>, density: f64| match property {
            StyleProperty::Constant(color) => Some(ExpressionColor::from(color.clone())),
            StyleProperty::Expression(property) => {
                let context = EvaluationContext {
                    heatmap_density: density,
                    ..EvaluationContext::default()
                };
                match property.expression().evaluate(&context) {
                    Ok(Value::Color(color)) => Some(color),
                    _ => None,
                }
            }
            StyleProperty::Unsupported(_) => None,
        };
        let default = default_color();
        let property = self.heatmap_color.as_ref().unwrap_or(&default);
        (0..RAMP_TEXELS)
            .map(|texel| {
                let density = texel as f64 / (RAMP_TEXELS - 1) as f64;
                let color = colour_at(property, density)
                    .or_else(|| colour_at(&default, density))
                    .map_or([0.0; 4], |color| color.premultiplied());
                color.map(|channel| (channel * 255.0).round() as u8)
            })
            .collect()
    }
}

fn default_color() -> StyleProperty<Color> {
    match serde_json::from_str(DEFAULT_COLOR) {
        Ok(json) => StyleProperty::parse(&json),
        Err(_) => StyleProperty::Constant(Color::new(0.0, 0.0, 0.0, 0.0)),
    }
}

#[cfg(test)]
mod tests;
