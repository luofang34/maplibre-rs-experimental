//! Root light properties used by atmosphere and lit globe rendering.

use cgmath::{InnerSpace, Vector3};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::style::{
    expression::{LegacyPropertySpec, PropertyKind, Value},
    layer::StyleProperty,
    property::PropertyValue,
};

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

impl PropertyValue for LightAnchor {
    fn spec() -> LegacyPropertySpec {
        LegacyPropertySpec::stepped(PropertyKind::Enum(vec![
            "viewport".to_owned(),
            "map".to_owned(),
        ]))
    }

    fn from_value(value: &Value) -> Option<Self> {
        match value.as_str()? {
            "viewport" => Some(Self::Viewport),
            "map" => Some(Self::Map),
            _ => None,
        }
    }
}

/// Root light configuration relevant to globe rendering.
#[derive(Clone, Debug, Serialize)]
pub struct LightSpecification {
    /// Coordinate frame for the light position.
    pub anchor: StyleProperty<LightAnchor>,
    /// Spherical position as radius, azimuth, and polar angle.
    pub position: StyleProperty<[f64; 3]>,
    /// Colour of the light; extruded polygons are lit with it, white by default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<StyleProperty<csscolorparser::Color>>,
    /// Strength of the light in 0..=1; 0.5 by default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intensity: Option<StyleProperty<f32>>,
}

impl LightSpecification {
    /// Parses a `light` object directly from JSON.
    pub(crate) fn from_json_value(value: &serde_json::Value) -> Result<Self, serde_json::Error> {
        let Some(map) = value.as_object() else {
            return Err(serde::de::Error::custom("light must be a JSON object"));
        };
        Ok(Self {
            anchor: map
                .get("anchor")
                .map_or_else(default_anchor_property, StyleProperty::<LightAnchor>::parse),
            position: map
                .get("position")
                .map_or_else(default_position_property, StyleProperty::<[f64; 3]>::parse),
            color: parse_optional_color_prop(map, "color"),
            intensity: parse_optional_f32_prop(map, "intensity"),
        })
    }
}

impl<'de> Deserialize<'de> for LightSpecification {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        Self::from_json_value(&value).map_err(serde::de::Error::custom)
    }
}

impl Default for LightSpecification {
    fn default() -> Self {
        Self {
            anchor: default_anchor_property(),
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
        let (x, y, z) = spherical_coords_to_xyz(radius, azimuth, polar);
        let (x, y) = match self.anchor_at(zoom) {
            LightAnchor::Map => (x, y),
            LightAnchor::Viewport => rotate_xy_by_bearing(x, y, bearing_radians),
        };
        Ok(ExtrusionLight {
            position: [x as f32, y as f32, z as f32],
            color: eval_light_color(self.color.as_ref(), zoom),
            intensity: eval_light_intensity(self.intensity.as_ref(), zoom),
        })
    }

    /// The frame the light is anchored to at `zoom`; the viewport where it cannot be evaluated,
    /// as the specification's default.
    pub fn anchor_at(&self, zoom: f64) -> LightAnchor {
        self.anchor.evaluate_at_zoom(zoom).unwrap_or_default()
    }

    /// Evaluates the light's normalized sun direction before any map-frame camera transform.
    pub(crate) fn evaluate_sun_direction(
        &self,
        zoom: f64,
    ) -> Result<(LightAnchor, Vector3<f64>), LightError> {
        let position = evaluate_position(&self.position, zoom)?;
        let cartesian = spherical_to_cartesian(position)?;
        Ok((self.anchor_at(zoom), normalize(-cartesian)?))
    }
}

fn default_anchor_property() -> StyleProperty<LightAnchor> {
    StyleProperty::Constant(LightAnchor::Viewport)
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

fn spherical_coords_to_xyz(
    radius: f64,
    azimuth_degrees: f64,
    polar_degrees: f64,
) -> (f64, f64, f64) {
    let azimuth = (azimuth_degrees + 90.0).to_radians();
    let polar = polar_degrees.to_radians();
    (
        radius * azimuth.cos() * polar.sin(),
        radius * azimuth.sin() * polar.sin(),
        radius * polar.cos(),
    )
}

fn rotate_xy_by_bearing(x: f64, y: f64, bearing_radians: f64) -> (f64, f64) {
    let (sin, cos) = bearing_radians.sin_cos();
    (x * cos - y * sin, x * sin + y * cos)
}

fn eval_light_color(
    property: Option<&StyleProperty<csscolorparser::Color>>,
    zoom: f64,
) -> [f32; 3] {
    property
        .and_then(|color| color.evaluate_at_zoom(zoom))
        .map_or([1.0; 3], |color| [color.r, color.g, color.b])
}

fn eval_light_intensity(property: Option<&StyleProperty<f32>>, zoom: f64) -> f32 {
    property
        .and_then(|intensity| intensity.evaluate_at_zoom(zoom))
        .unwrap_or(0.5)
}

fn parse_optional_color_prop(
    map: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<StyleProperty<csscolorparser::Color>> {
    map.get(key)
        .filter(|v| !v.is_null())
        .map(StyleProperty::<csscolorparser::Color>::parse)
}

fn parse_optional_f32_prop(
    map: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<StyleProperty<f32>> {
    map.get(key)
        .filter(|v| !v.is_null())
        .map(StyleProperty::<f32>::parse)
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
    let (x, y, z) = spherical_coords_to_xyz(radius, azimuth_degrees, polar_degrees);
    normalize(Vector3::new(x, y, z))
}

fn normalize(direction: Vector3<f64>) -> Result<Vector3<f64>, LightError> {
    (direction.magnitude2().is_finite() && direction.magnitude2() > MIN_DIRECTION_LENGTH_SQUARED)
        .then(|| direction.normalize())
        .ok_or(LightError::InvalidPosition)
}

#[cfg(test)]
mod tests;
