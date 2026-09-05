//! Paint properties of `hillshade` and `color-relief` layers, and the per-frame values the
//! DEM shaders take from them.

use serde::{Deserialize, Serialize};

use crate::style::{
    expression::{Color, EvaluationContext, Expression, Global},
    property::{ColorList, NumberList, StyleProperty},
};

/// Largest number of light sources a hillshade layer shades with at once.
pub const MAX_LIGHTS: usize = 4;
/// Largest number of stops a colour relief ramp carries.
pub const MAX_RAMP_STOPS: usize = 64;

/// How a hillshade layer turns slopes into shading.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum HillshadeMethod {
    /// The GL JS shading with an accent colour on steep slopes.
    #[default]
    #[serde(rename = "standard")]
    Standard,
    /// GDAL's hillshade, shadow to highlight around a transparent middle.
    #[serde(rename = "basic")]
    Basic,
    /// GDAL's combined shading, slope and aspect together.
    #[serde(rename = "combined")]
    Combined,
    /// GDAL's Igor shading, aspect only.
    #[serde(rename = "igor")]
    Igor,
    /// The basic shading averaged over several lights.
    #[serde(rename = "multidirectional")]
    Multidirectional,
}

impl HillshadeMethod {
    /// The method's number in the shader.
    pub fn shader_code(self) -> u32 {
        match self {
            Self::Standard => 0,
            Self::Combined => 1,
            Self::Igor => 2,
            Self::Multidirectional => 3,
            Self::Basic => 4,
        }
    }
}

/// Whether light directions follow the map or the viewport.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum IlluminationAnchor {
    /// Directions turn with the map's bearing.
    #[serde(rename = "map")]
    Map,
    /// Directions are fixed on the screen.
    #[default]
    #[serde(rename = "viewport")]
    Viewport,
}

/// Paint of a `hillshade` layer; an absent property means the specification default.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct HillshadePaint {
    /// Light azimuths in degrees clockwise from north, one per light.
    #[serde(rename = "hillshade-illumination-direction", default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hillshade_illumination_direction: Option<StyleProperty<NumberList>>,
    /// Light altitudes in degrees above the horizon, one per light.
    #[serde(rename = "hillshade-illumination-altitude", default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hillshade_illumination_altitude: Option<StyleProperty<NumberList>>,
    /// Whether the directions follow the map or the viewport.
    #[serde(rename = "hillshade-illumination-anchor", default)]
    pub hillshade_illumination_anchor: IlluminationAnchor,
    /// Shading intensity in `0..=1`.
    #[serde(rename = "hillshade-exaggeration", default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hillshade_exaggeration: Option<StyleProperty<f32>>,
    /// Colour of slopes facing away from the light, one per light.
    #[serde(rename = "hillshade-shadow-color", default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hillshade_shadow_color: Option<StyleProperty<ColorList>>,
    /// Colour of slopes facing the light, one per light.
    #[serde(rename = "hillshade-highlight-color", default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hillshade_highlight_color: Option<StyleProperty<ColorList>>,
    /// Colour of steep slopes in the standard method.
    #[serde(rename = "hillshade-accent-color", default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hillshade_accent_color: Option<StyleProperty<csscolorparser::Color>>,
    /// The shading method.
    #[serde(rename = "hillshade-method", default)]
    pub hillshade_method: HillshadeMethod,
}

/// The lights of a hillshade layer at one zoom, with every list padded to the same length as
/// GL JS `getIlluminationProperties` does.
#[derive(Clone, Debug, PartialEq)]
pub struct Illumination {
    /// Azimuths in radians, with the map bearing folded in for viewport-anchored layers.
    pub azimuths: Vec<f32>,
    /// Altitudes in radians.
    pub altitudes: Vec<f32>,
    /// Premultiplied shadow colours.
    pub shadows: Vec<[f32; 4]>,
    /// Premultiplied highlight colours.
    pub highlights: Vec<[f32; 4]>,
}

impl HillshadePaint {
    /// The lights at `zoom`, with directions turned by `bearing_radians` when the layer is
    /// anchored to the viewport.
    pub fn illumination(&self, zoom: f64, bearing_radians: f64) -> Illumination {
        let numbers = |property: &Option<StyleProperty<NumberList>>, default: f64| {
            property
                .as_ref()
                .and_then(|property| property.evaluate_at_zoom(zoom))
                .map_or(vec![default], |list| list.0)
        };
        let colors = |property: &Option<StyleProperty<ColorList>>, default: Color| {
            property
                .as_ref()
                .and_then(|property| property.evaluate_at_zoom(zoom))
                .map_or(vec![default], |list| list.0)
        };
        let mut directions = numbers(&self.hillshade_illumination_direction, 335.0);
        let mut altitudes = numbers(&self.hillshade_illumination_altitude, 45.0);
        let mut shadows = colors(&self.hillshade_shadow_color, Color::new(0.0, 0.0, 0.0, 1.0));
        let mut highlights = colors(
            &self.hillshade_highlight_color,
            Color::new(1.0, 1.0, 1.0, 1.0),
        );
        let count = directions
            .len()
            .max(altitudes.len())
            .max(shadows.len())
            .max(highlights.len())
            .clamp(1, MAX_LIGHTS);
        pad(&mut directions, count);
        pad(&mut altitudes, count);
        pad(&mut shadows, count);
        pad(&mut highlights, count);
        let turn = match self.hillshade_illumination_anchor {
            IlluminationAnchor::Viewport => bearing_radians,
            IlluminationAnchor::Map => 0.0,
        };
        Illumination {
            azimuths: directions
                .iter()
                .map(|degrees| (degrees.to_radians() + turn) as f32)
                .collect(),
            altitudes: altitudes
                .iter()
                .map(|degrees| degrees.to_radians() as f32)
                .collect(),
            shadows: shadows.iter().map(premultiplied).collect(),
            highlights: highlights.iter().map(premultiplied).collect(),
        }
    }

    /// Shading intensity at `zoom`.
    pub fn exaggeration_at(&self, zoom: f64) -> f32 {
        self.hillshade_exaggeration
            .as_ref()
            .and_then(|property| property.evaluate_at_zoom(zoom))
            .unwrap_or(0.5)
    }

    /// Premultiplied accent colour at `zoom`.
    pub fn accent_at(&self, zoom: f64) -> [f32; 4] {
        self.hillshade_accent_color
            .as_ref()
            .and_then(|property| property.evaluate_at_zoom(zoom))
            .map_or([0.0, 0.0, 0.0, 1.0], |color| premultiplied(&color.into()))
    }
}

fn pad<T: Clone>(list: &mut Vec<T>, count: usize) {
    list.truncate(count);
    while list.len() < count {
        let last = list[list.len() - 1].clone();
        list.push(last);
    }
}

fn premultiplied(color: &Color) -> [f32; 4] {
    let [r, g, b, a] = color.premultiplied();
    [r as f32, g as f32, b as f32, a as f32]
}

/// Paint of a `color-relief` layer.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct ColorReliefPaint {
    /// Opacity of the relief.
    #[serde(rename = "color-relief-opacity", default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_relief_opacity: Option<StyleProperty<f32>>,
    /// Colour as a function of `["elevation"]`.
    #[serde(rename = "color-relief-color", default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_relief_color: Option<StyleProperty<csscolorparser::Color>>,
}

impl ColorReliefPaint {
    /// Opacity at `zoom`.
    pub fn opacity_at(&self, zoom: f64) -> f32 {
        self.color_relief_opacity
            .as_ref()
            .and_then(|property| property.evaluate_at_zoom(zoom))
            .unwrap_or(1.0)
    }

    /// The colour ramp as ascending `(elevation, premultiplied colour)` stops.
    ///
    /// An `interpolate` over elevation gives its stops; a `step` gives two stops per edge so
    /// the ramp keeps its hard edges; anything else is one colour for every elevation.
    pub fn ramp(&self) -> Vec<(f32, [f32; 4])> {
        let Some(property) = &self.color_relief_color else {
            return Vec::new();
        };
        let evaluate_at = |expression: &Expression, elevation: f64| {
            let context = EvaluationContext {
                elevation,
                ..EvaluationContext::default()
            };
            match expression.evaluate(&context) {
                Ok(crate::style::expression::Value::Color(color)) => Some(premultiplied(&color)),
                _ => None,
            }
        };
        let mut ramp = Vec::new();
        match property {
            StyleProperty::Constant(color) => {
                ramp.push((0.0, premultiplied(&(color.clone().into()))));
            }
            StyleProperty::Expression(property) => match property.expression() {
                Expression::Interpolate { input, stops, .. }
                    if matches!(**input, Expression::Global(Global::Elevation)) =>
                {
                    for (elevation, output) in stops {
                        if let Some(color) = evaluate_at(output, *elevation) {
                            ramp.push((*elevation as f32, color));
                        }
                    }
                }
                Expression::Step { input, stops, .. }
                    if matches!(**input, Expression::Global(Global::Elevation)) =>
                {
                    for (index, (elevation, output)) in stops.iter().enumerate() {
                        let Some(color) = evaluate_at(output, elevation.max(-1e9)) else {
                            continue;
                        };
                        if index > 0 {
                            if let Some(previous) = ramp.last().map(|(_, color)| *color) {
                                ramp.push((*elevation as f32, previous));
                            }
                        }
                        ramp.push((elevation.max(-1e9) as f32, color));
                    }
                }
                expression => {
                    if let Some(color) = evaluate_at(expression, 0.0) {
                        ramp.push((0.0, color));
                    }
                }
            },
            StyleProperty::Unsupported(_) => {}
        }
        ramp.truncate(MAX_RAMP_STOPS);
        ramp
    }
}

#[cfg(test)]
mod tests;
