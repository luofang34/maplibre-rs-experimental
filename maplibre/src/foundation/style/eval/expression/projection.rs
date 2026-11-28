//! Root `projection` style specification and zoom-driven projection expressions.

use serde::{Deserialize, Serialize};

mod expr;
mod wire;

use expr::ProjectionExpressionKind;
pub use expr::{
    InterpolationCurve, NamedProjection, ProjectionExpression, ProjectionExpressionError,
    ProjectionStop,
};

/// GL JS implements the `globe` preset as a blend from vertical perspective at zoom 11 to
/// Mercator at zoom 12, and its render goldens at zoom 11 expect a full globe. The style
/// specification's documentation shows the same blend starting at zoom 10; the renderer follows
/// the implementation and its goldens.
const GLOBE_TRANSITION_START_ZOOM: f64 = 11.0;
const GLOBE_TRANSITION_END_ZOOM: f64 = 12.0;

/// Returns the `globe` preset's vertical-perspective weight at a continuous zoom.
pub fn transition_for_zoom(zoom: f64) -> f32 {
    if !zoom.is_finite() {
        return 0.0;
    }
    ((GLOBE_TRANSITION_END_ZOOM - zoom) / (GLOBE_TRANSITION_END_ZOOM - GLOBE_TRANSITION_START_ZOOM))
        .clamp(0.0, 1.0) as f32
}

/// A projection selected by a style.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ProjectionType {
    /// Render Web Mercator tiles on a plane.
    #[default]
    Mercator,
    /// Render Web Mercator tiles on a globe.
    Globe,
    /// Keep vertical-perspective globe rendering at every zoom level.
    VerticalPerspective,
    /// Evaluate a validated zoom expression or explicit transition.
    Expression(ProjectionExpression),
}

impl ProjectionType {
    /// Returns the active globe weight at the given continuous zoom.
    pub fn globe_transition(&self, zoom: f64) -> f32 {
        match self {
            Self::Mercator => 0.0,
            Self::Globe => transition_for_zoom(zoom),
            Self::VerticalPerspective => 1.0,
            Self::Expression(expression) => expression.globe_transition(zoom),
        }
    }

    /// Returns whether globe rendering contributes to the current frame.
    pub fn uses_globe_rendering(&self, zoom: f64) -> bool {
        self.globe_transition(zoom) > 0.0
    }

    /// Creates an explicit transition between two projections.
    pub fn transition(
        from: NamedProjection,
        to: NamedProjection,
        transition: f64,
    ) -> Result<Self, ProjectionExpressionError> {
        ProjectionExpression::transition(from, to, transition).map(Self::Expression)
    }

    /// Creates a zoom interpolation from validated stops.
    pub fn interpolate(
        curve: InterpolationCurve,
        stops: Vec<ProjectionStop>,
    ) -> Result<Self, ProjectionExpressionError> {
        ProjectionExpression::interpolate(curve, stops).map(Self::Expression)
    }

    /// Creates a piecewise-constant zoom expression.
    pub fn step(
        default: NamedProjection,
        stops: Vec<ProjectionStop>,
    ) -> Result<Self, ProjectionExpressionError> {
        ProjectionExpression::step(default, stops).map(Self::Expression)
    }
}

/// The root-level projection section of a style document.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ProjectionSpecification {
    /// Projection used to display the map.
    #[serde(rename = "type")]
    pub projection_type: ProjectionType,
}

impl ProjectionSpecification {
    /// Parses a root `projection` object directly from a JSON [`serde_json::Value`].
    pub(crate) fn from_json_value(value: &serde_json::Value) -> Result<Self, serde_json::Error> {
        let Some(map) = value.as_object() else {
            return Err(serde::de::Error::custom("projection must be an object"));
        };
        let Some(ty) = map.get("type") else {
            return Err(serde::de::Error::missing_field("type"));
        };
        let projection_type = ProjectionType::parse_json(ty).map_err(serde::de::Error::custom)?;
        Ok(Self { projection_type })
    }
}

impl<'de> Deserialize<'de> for ProjectionSpecification {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        Self::from_json_value(&value).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests;
