//! Root light properties used by atmosphere and lit globe rendering.

use cgmath::{InnerSpace, Vector3};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::layer::StyleProperty;
use crate::projection::globe::camera::GlobeCameraState;

const DEFAULT_POSITION: [f64; 3] = [1.15, 210.0, 30.0];
const MIN_DIRECTION_LENGTH_SQUARED: f64 = 1e-24;

/// Coordinate frame in which the root light position is expressed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LightAnchor {
    /// Light remains fixed relative to the viewport.
    #[default]
    Viewport,
    /// Light rotates with the geographic map.
    Map,
}

/// Root light configuration relevant to globe rendering.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LightSpecification {
    /// Coordinate frame for the light position.
    #[serde(default)]
    pub anchor: LightAnchor,
    /// Spherical position as radius, azimuth, and polar angle.
    #[serde(default = "default_position_property")]
    pub position: StyleProperty<[f64; 3]>,
    /// Colour of the light; extruded polygons are lit with it, white by default.
    #[serde(
        default,
        deserialize_with = "StyleProperty::<csscolorparser::Color>::deserialize_color_or_none"
    )]
    pub color: Option<StyleProperty<csscolorparser::Color>>,
    /// Strength of the light in 0..=1; 0.5 by default.
    #[serde(
        default,
        deserialize_with = "StyleProperty::<f32>::deserialize_f32_or_none"
    )]
    pub intensity: Option<StyleProperty<f32>>,
}

impl Default for LightSpecification {
    fn default() -> Self {
        Self {
            anchor: LightAnchor::Viewport,
            position: default_position_property(),
            color: None,
            intensity: None,
        }
    }
}

/// Invalid root light data used by the atmosphere pass.
#[derive(Clone, Copy, Debug, Error, PartialEq)]
pub enum LightError {
    /// The light position cannot produce a finite direction.
    #[error("light position must contain a positive radius and finite angles")]
    InvalidPosition,
    /// The evaluated light expression has an unsupported form.
    #[error("light position expression cannot be evaluated at zoom {zoom}")]
    UnsupportedPositionExpression {
        /// Zoom at which evaluation failed.
        zoom: f64,
    },
    /// A valid light direction collapsed under the camera transform.
    #[error("light direction cannot be transformed into the current view")]
    InvalidViewDirection,
}

/// The light as the extrusion shader takes it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ExtrusionLight {
    /// Position on the sphere of the given radius, in map axes with y pointing south.
    pub position: [f32; 3],
    /// Linear-channel colour of the light.
    pub color: [f32; 3],
    /// Strength in 0..=1.
    pub intensity: f32,
}

impl LightSpecification {
    /// Evaluates the light at `zoom` for a view turned by `bearing_radians`, as GL JS lights
    /// extrusions: a viewport-anchored light turns against the map.
    pub fn extrusion_light(
        &self,
        zoom: f64,
        bearing_radians: f64,
    ) -> Result<ExtrusionLight, LightError> {
        let [radius, azimuth, polar] = evaluate_position(&self.position, zoom)?;
        if !radius.is_finite() || !azimuth.is_finite() || !polar.is_finite() {
            return Err(LightError::InvalidPosition);
        }
        let azimuth = (azimuth + 90.0).to_radians();
        let polar = polar.to_radians();
        let (x, y, z) = (
            radius * azimuth.cos() * polar.sin(),
            radius * azimuth.sin() * polar.sin(),
            radius * polar.cos(),
        );
        let (x, y) = match self.anchor {
            LightAnchor::Map => (x, y),
            LightAnchor::Viewport => {
                let (sin, cos) = bearing_radians.sin_cos();
                (x * cos - y * sin, x * sin + y * cos)
            }
        };
        let color = self
            .color
            .as_ref()
            .and_then(|color| color.evaluate_at_zoom(zoom))
            .map_or([1.0; 3], |color| {
                [color.r as f32, color.g as f32, color.b as f32]
            });
        let intensity = self
            .intensity
            .as_ref()
            .and_then(|intensity| intensity.evaluate_at_zoom(zoom))
            .unwrap_or(0.5);
        Ok(ExtrusionLight {
            position: [x as f32, y as f32, z as f32],
            color,
            intensity,
        })
    }

    /// Evaluates the light and returns the sun direction in camera-view axes.
    pub fn sun_direction_in_view(
        &self,
        camera: &GlobeCameraState,
        zoom: f64,
    ) -> Result<Vector3<f64>, LightError> {
        let position = evaluate_position(&self.position, zoom)?;
        let cartesian = spherical_to_cartesian(position)?;
        let sun = -cartesian;
        match self.anchor {
            LightAnchor::Viewport => normalize(sun),
            LightAnchor::Map => camera
                .world_direction_to_view(sun)
                .ok_or(LightError::InvalidViewDirection),
        }
    }
}

fn default_position_property() -> StyleProperty<[f64; 3]> {
    StyleProperty::Constant(DEFAULT_POSITION)
}

fn evaluate_position(
    property: &StyleProperty<[f64; 3]>,
    zoom: f64,
) -> Result<[f64; 3], LightError> {
    property
        .evaluate_at_zoom(zoom)
        .ok_or(LightError::UnsupportedPositionExpression { zoom })
}

fn spherical_to_cartesian(position: [f64; 3]) -> Result<Vector3<f64>, LightError> {
    let [radius, azimuth_degrees, polar_degrees] = position;
    if !radius.is_finite()
        || radius <= 0.0
        || !azimuth_degrees.is_finite()
        || !polar_degrees.is_finite()
    {
        return Err(LightError::InvalidPosition);
    }
    let azimuth = (azimuth_degrees + 90.0).to_radians();
    let polar = polar_degrees.to_radians();
    normalize(Vector3::new(
        radius * azimuth.cos() * polar.sin(),
        radius * azimuth.sin() * polar.sin(),
        radius * polar.cos(),
    ))
}

fn normalize(direction: Vector3<f64>) -> Result<Vector3<f64>, LightError> {
    (direction.magnitude2().is_finite() && direction.magnitude2() > MIN_DIRECTION_LENGTH_SQUARED)
        .then(|| direction.normalize())
        .ok_or(LightError::InvalidPosition)
}

#[cfg(test)]
mod tests;
