//! Evaluation of a parsed expression for one feature at one zoom.

use std::collections::HashMap;

use rustc_hash::FxHashMap;

use super::{
    ast::Expression,
    geometry::{Geometry, GEOMETRY_PROPERTY},
    value::Value,
};

mod coercions;
mod dispatch;
mod error;
mod images;
mod operations;

pub use error::EvaluationError;
use operations::*;

/// The typed properties of a feature.
pub type FeatureProperties = FxHashMap<String, Value>;

/// What an expression can observe while it is evaluated.
#[derive(Clone, Copy, Default)]
pub struct EvaluationContext<'a> {
    /// Zoom the expression is evaluated at.
    pub zoom: f64,
    /// Terrain elevation in metres, for `elevation`.
    pub elevation: f64,
    /// Density of a heatmap at the colour being looked up, for `heatmap-density`.
    pub heatmap_density: f64,
    /// How far along its line the colour being looked up lies, for `line-progress`.
    pub line_progress: f64,
    /// Properties of the feature; none when the expression is not evaluated for a feature.
    pub properties: Option<&'a FeatureProperties>,
    /// Geometry kind of the feature, as `geometry-type` reports it.
    pub geometry_type: Option<&'a str>,
    /// Id of the feature, when the source assigned one.
    pub id: Option<&'a Value>,
    /// Accumulated cluster property value read by `accumulated`; `None` outside cluster reduction.
    pub accumulated: Option<&'a Value>,
    /// Map-level state read by `global-state`; the map keeps none, so it is normally absent.
    pub global_state: Option<&'a FeatureProperties>,
    /// The images a layout can draw, which `image` expressions are checked against; absent when
    /// every image is assumed to exist.
    pub available_images: Option<&'a dyn ImageSet>,
}

impl std::fmt::Debug for EvaluationContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EvaluationContext")
            .field("zoom", &self.zoom)
            .field("elevation", &self.elevation)
            .field("heatmap_density", &self.heatmap_density)
            .field("line_progress", &self.line_progress)
            .field("properties", &self.properties)
            .field("geometry_type", &self.geometry_type)
            .field("id", &self.id)
            .field("accumulated", &self.accumulated)
            .field("global_state", &self.global_state)
            .field(
                "available_images",
                &self.available_images.map(|_| "<images>"),
            )
            .finish()
    }
}

/// A set of image names.
pub trait ImageSet {
    /// Whether an image of that name exists.
    fn contains_image(&self, name: &str) -> bool;
}

impl<V, S: std::hash::BuildHasher> ImageSet for HashMap<String, V, S> {
    fn contains_image(&self, name: &str) -> bool {
        self.contains_key(name)
    }
}

impl<'a> EvaluationContext<'a> {
    /// A context without a feature, for zoom-driven properties.
    pub fn at_zoom(zoom: f64) -> Self {
        Self {
            zoom,
            ..Self::default()
        }
    }

    /// A context for a feature.
    pub fn for_feature(zoom: f64, properties: &'a FeatureProperties) -> Self {
        Self {
            zoom,
            properties: Some(properties),
            ..Self::default()
        }
    }
}

/// The geometry a feature carries in its properties, when it carries one.
fn feature_geometry(context: &EvaluationContext<'_>) -> Option<Geometry> {
    let text = context.properties?.get(GEOMETRY_PROPERTY)?.as_str()?;
    Geometry::from_geojson(&serde_json::from_slice(text.as_bytes()).ok()?)
}

type Result<T> = std::result::Result<T, EvaluationError>;

impl Expression {
    /// Evaluates the expression.
    pub fn evaluate(&self, context: &EvaluationContext) -> Result<Value> {
        if let Some(result) = dispatch::eval_leaf_or_feature(self, context) {
            return result;
        }
        if let Some(result) = dispatch::eval_control_or_logic(self, context) {
            return result;
        }
        dispatch::eval_other(self, context)
    }
}
