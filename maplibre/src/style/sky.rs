//! Sky properties used by globe rendering.

use serde::{Deserialize, Serialize};

use super::layer::StyleProperty;

/// Root-level sky configuration.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SkySpecification {
    /// Opacity of atmospheric scattering around a globe.
    #[serde(
        rename = "atmosphere-blend",
        default,
        deserialize_with = "StyleProperty::<f32>::deserialize_f32_or_none",
        skip_serializing_if = "Option::is_none"
    )]
    pub atmosphere_blend: Option<StyleProperty<f32>>,
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
}

#[cfg(test)]
mod tests;
