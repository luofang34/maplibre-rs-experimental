//! The root `sky` of a style: the colours above the horizon, the fog on distant terrain and
//! the atmosphere of a globe.

use serde::{Deserialize, Serialize};

use crate::style::{expression::Color, layer::StyleProperty};

/// Root-level sky configuration; an absent property means the specification default.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SkySpecification {
    /// Colour of the sky away from the horizon.
    #[serde(rename = "sky-color", default, skip_serializing_if = "Option::is_none")]
    pub sky_color: Option<StyleProperty<csscolorparser::Color>>,
    /// Colour of the sky at the horizon.
    #[serde(
        rename = "horizon-color",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub horizon_color: Option<StyleProperty<csscolorparser::Color>>,
    /// Colour of the fog on distant terrain.
    #[serde(rename = "fog-color", default, skip_serializing_if = "Option::is_none")]
    pub fog_color: Option<StyleProperty<csscolorparser::Color>>,
    /// Fog depth, in `0..=1` of the range from the map center to the far plane, at which the
    /// terrain starts to blend into the fog.
    #[serde(
        rename = "fog-ground-blend",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub fog_ground_blend: Option<StyleProperty<f32>>,
    /// Fog depth at which the fog colour starts to blend into the horizon colour.
    #[serde(
        rename = "horizon-fog-blend",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub horizon_fog_blend: Option<StyleProperty<f32>>,
    /// Height of the band above the horizon, as a share of half the viewport, over which the
    /// horizon colour blends into the sky colour.
    #[serde(
        rename = "sky-horizon-blend",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub sky_horizon_blend: Option<StyleProperty<f32>>,
    /// Opacity of atmospheric scattering around a globe.
    #[serde(
        rename = "atmosphere-blend",
        default,
        deserialize_with = "StyleProperty::<f32>::deserialize_f32_or_none",
        skip_serializing_if = "Option::is_none"
    )]
    pub atmosphere_blend: Option<StyleProperty<f32>>,
}

/// The sky at one zoom and pitch, as the shaders take it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyColors {
    /// Premultiplied sky colour.
    pub sky: [f32; 4],
    /// Premultiplied horizon colour.
    pub horizon: [f32; 4],
    /// Premultiplied fog colour.
    pub fog: [f32; 4],
    /// Where the terrain starts to blend into the fog.
    pub fog_ground_blend: f32,
    /// Where the fog starts to blend into the horizon colour.
    pub horizon_fog_blend: f32,
    /// Share of half the viewport over which the horizon blends into the sky.
    pub sky_horizon_blend: f32,
}

impl SkySpecification {
    /// Evaluates atmospheric opacity at a continuous camera zoom.
    pub fn atmosphere_blend_at_zoom(&self, zoom: f64) -> f32 {
        let value = self
            .atmosphere_blend
            .as_ref()
            .and_then(|blend| blend.evaluate_at_zoom(zoom))
            .unwrap_or(0.0);
        if value.is_finite() {
            value.clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// The sky and fog colours and blends at `zoom`, with the specification defaults filled in.
    pub fn colors_at(&self, zoom: f64) -> SkyColors {
        let color = |property: &Option<StyleProperty<csscolorparser::Color>>, default: Color| {
            let color = property
                .as_ref()
                .and_then(|property| property.evaluate_at_zoom(zoom))
                .map_or(default, Color::from);
            let [r, g, b, a] = color.premultiplied();
            [r as f32, g as f32, b as f32, a as f32]
        };
        let number = |property: &Option<StyleProperty<f32>>, default: f32| {
            property
                .as_ref()
                .and_then(|property| property.evaluate_at_zoom(zoom))
                .filter(|value| value.is_finite())
                .unwrap_or(default)
                .clamp(0.0, 1.0)
        };
        SkyColors {
            sky: color(&self.sky_color, Color::new(0.533, 0.776, 0.988, 1.0)),
            horizon: color(&self.horizon_color, Color::new(1.0, 1.0, 1.0, 1.0)),
            fog: color(&self.fog_color, Color::new(1.0, 1.0, 1.0, 1.0)),
            fog_ground_blend: number(&self.fog_ground_blend, 0.5),
            horizon_fog_blend: number(&self.horizon_fog_blend, 0.8),
            sky_horizon_blend: number(&self.sky_horizon_blend, 0.8),
        }
    }

    /// How much of the fog shows at a pitch, as GL JS `calculateFogBlendOpacity`: the fog is
    /// drawn from the far plane to the map center without knowing the horizon, so it fades in
    /// only as the horizon comes into view between 60 and 70 degrees of pitch.
    pub fn fog_blend_opacity(pitch_degrees: f64) -> f32 {
        if pitch_degrees < 60.0 {
            0.0
        } else if pitch_degrees < 70.0 {
            ((pitch_degrees - 60.0) / 10.0) as f32
        } else {
            1.0
        }
    }
}

#[cfg(test)]
mod tests;
