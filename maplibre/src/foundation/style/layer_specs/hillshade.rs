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
    /// DEM resampling mode (`"linear"` default or `"nearest"`).
    #[serde(
        rename = "hillshade-resampling",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub hillshade_resampling: Option<crate::style::property::RasterResampling>,
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
        let mut directions = eval_number_list_or_default(
            self.hillshade_illumination_direction.as_ref(),
            zoom,
            335.0,
        );
        let mut altitudes =
            eval_number_list_or_default(self.hillshade_illumination_altitude.as_ref(), zoom, 45.0);
        let mut shadows = eval_color_list_or_default(
            self.hillshade_shadow_color.as_ref(),
            zoom,
            Color::new(0.0, 0.0, 0.0, 1.0),
        );
        let mut highlights = eval_color_list_or_default(
            self.hillshade_highlight_color.as_ref(),
            zoom,
            Color::new(1.0, 1.0, 1.0, 1.0),
        );
        let count = directions
            .len()
            .max(altitudes.len())
            .max(shadows.len())
            .max(highlights.len())
            .clamp(1, MAX_LIGHTS);
        pad(&mut directions, count, 335.0);
        pad(&mut altitudes, count, 45.0);
        pad(&mut shadows, count, Color::new(0.0, 0.0, 0.0, 1.0));
        pad(&mut highlights, count, Color::new(1.0, 1.0, 1.0, 1.0));
        let turn = anchor_turn_radians(
            matches!(
                self.hillshade_illumination_anchor,
                IlluminationAnchor::Viewport
            ),
            bearing_radians,
        );
        Illumination {
            azimuths: degrees_list_to_radians(&directions, turn),
            altitudes: degrees_list_to_radians(&altitudes, 0.0),
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

    /// Whether DEM texels are sampled without interpolation at `zoom`.
    pub fn is_nearest_at(&self, zoom: f64) -> bool {
        self.hillshade_resampling
            .as_ref()
            .is_some_and(|resampling| resampling.is_nearest_at(zoom))
    }
}

fn eval_number_list_or_default(
    property: Option<&StyleProperty<NumberList>>,
    zoom: f64,
    default: f64,
) -> Vec<f64> {
    property
        .and_then(|property| property.evaluate_at_zoom(zoom))
        .map(|list| list.0)
        .filter(|list| !list.is_empty())
        .unwrap_or_else(|| vec![default])
}

fn eval_color_list_or_default(
    property: Option<&StyleProperty<ColorList>>,
    zoom: f64,
    default: Color,
) -> Vec<Color> {
    property
        .and_then(|property| property.evaluate_at_zoom(zoom))
        .map(|list| list.0)
        .filter(|list| !list.is_empty())
        .unwrap_or_else(|| vec![default])
}

fn anchor_turn_radians(viewport_anchored: bool, bearing_radians: f64) -> f64 {
    if viewport_anchored {
        bearing_radians
    } else {
        0.0
    }
}

fn degrees_list_to_radians(degrees: &[f64], turn: f64) -> Vec<f32> {
    degrees
        .iter()
        .map(|deg| (deg.to_radians() + turn) as f32)
        .collect()
}

fn pad<T: Clone>(list: &mut Vec<T>, count: usize, default: T) {
    if list.is_empty() {
        list.push(default);
    }
    list.truncate(count);
    if let Some(last) = list.last().cloned() {
        list.resize(count, last);
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
    /// DEM resampling mode (`"linear"` default or `"nearest"`).
    #[serde(
        rename = "color-relief-resampling",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub color_relief_resampling: Option<crate::style::property::RasterResampling>,
}

impl ColorReliefPaint {
    /// Opacity at `zoom`.
    pub fn opacity_at(&self, zoom: f64) -> f32 {
        self.color_relief_opacity
            .as_ref()
            .and_then(|property| property.evaluate_at_zoom(zoom))
            .unwrap_or(1.0)
    }

    /// Whether DEM elevations are sampled without bilinear interpolation at `zoom`.
    pub fn is_nearest_at(&self, zoom: f64) -> bool {
        self.color_relief_resampling
            .as_ref()
            .is_some_and(|resampling| resampling.is_nearest_at(zoom))
    }

    /// The colour ramp as ascending `(elevation, premultiplied colour)` stops.
    ///
    /// An `interpolate` over elevation gives its stops; a `step` gives two stops per edge so
    /// the ramp keeps its hard edges; anything else is one colour for every elevation.
    pub fn ramp(&self) -> Vec<(f32, [f32; 4])> {
        let Some(property) = &self.color_relief_color else {
            return Vec::new();
        };
        let mut ramp = match property {
            StyleProperty::Constant(color) => vec![(0.0, premultiplied(&((*color).into())))],
            StyleProperty::Expression(property) => match property.expression() {
                Expression::Interpolate { input, stops, .. }
                    if matches!(**input, Expression::Global(Global::Elevation)) =>
                {
                    collect_interpolated_ramp(stops)
                }
                Expression::Step { input, stops, .. }
                    if matches!(**input, Expression::Global(Global::Elevation)) =>
                {
                    collect_stepped_ramp(stops)
                }
                expression => eval_color_at_elevation(expression, 0.0)
                    .map(|color| vec![(0.0, color)])
                    .unwrap_or_default(),
            },
            StyleProperty::Unsupported(_) => Vec::new(),
        };
        ramp.truncate(MAX_RAMP_STOPS);
        ramp
    }
}

fn eval_color_at_elevation(expression: &Expression, elevation: f64) -> Option<[f32; 4]> {
    let context = EvaluationContext {
        elevation,
        ..EvaluationContext::default()
    };
    match expression.evaluate(&context) {
        Ok(crate::style::expression::Value::Color(color)) => Some(premultiplied(&color)),
        _ => None,
    }
}

fn collect_interpolated_ramp(stops: &[(f64, Expression)]) -> Vec<(f32, [f32; 4])> {
    stops
        .iter()
        .filter_map(|(elevation, output)| {
            eval_color_at_elevation(output, *elevation).map(|color| (*elevation as f32, color))
        })
        .collect()
}

fn collect_stepped_ramp(stops: &[(f64, Expression)]) -> Vec<(f32, [f32; 4])> {
    let mut ramp = Vec::new();
    for (index, (elevation, output)) in stops.iter().enumerate() {
        let Some(color) = eval_color_at_elevation(output, elevation.max(-1e9)) else {
            continue;
        };
        if index > 0 {
            if let Some(previous) = ramp.last().map(|(_, color)| *color) {
                ramp.push((*elevation as f32, previous));
            }
        }
        ramp.push((elevation.max(-1e9) as f32, color));
    }
    ramp
}

#[cfg(test)]
mod tests;
