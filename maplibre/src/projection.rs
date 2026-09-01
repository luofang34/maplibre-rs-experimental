//! Map projection definitions and coordinate conversion.

use serde::{Deserialize, Serialize};

pub mod globe;
pub mod renderer_data;

pub use globe::project_tile_coordinates_to_unit_sphere;

/// A projection selected by a style.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProjectionType {
    /// Render Web Mercator tiles on a plane.
    #[default]
    Mercator,
    /// Render Web Mercator tiles on a globe.
    Globe,
}

/// The root-level projection section of a style document.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectionSpecification {
    /// Projection used to display the map.
    #[serde(rename = "type")]
    pub projection_type: ProjectionType,
}

#[cfg(test)]
mod tests;
