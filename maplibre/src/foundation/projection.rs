//! Map projection definitions and coordinate conversion.

pub mod body;
pub mod camera_math;
pub mod globe;
pub mod renderer_data;
mod sphere;
pub(crate) mod tile_covering;
mod vertex;

pub use camera_math::{EyeFrustum, ViewProjectionError, OPENGL_TO_WGPU_MATRIX, REVERSED_Z};
pub use globe::project_tile_coordinates_to_unit_sphere;
pub use vertex::ShaderVertex;

pub use crate::style::expression::projection::{
    InterpolationCurve, NamedProjection, ProjectionExpression, ProjectionExpressionError,
    ProjectionSpecification, ProjectionStop, ProjectionType,
};

#[cfg(test)]
mod tests;

pub(crate) mod lod_history;
