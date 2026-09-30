//! Map style documents, layer properties and source configuration.

#![deny(missing_docs)]

use std::collections::HashMap;

pub use cint::*;
use serde::{Deserialize, Serialize};

pub mod circle;
pub mod expression;
pub mod filter;
pub mod hillshade;
pub mod layer;
pub mod light;
pub mod property;
pub mod sky;
pub mod source;
pub mod symbol;
pub mod terrain;
pub mod validation;

use crate::{
    projection::ProjectionSpecification,
    style::{layer::StyleLayer, source::Source},
};

/// Ordered layers, named data sources and initial view settings from a style document.
/// [`Self::validate`] checks layer rendering capabilities, not all style-spec constraints.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Style {
    /// Style specification version, normally 8; deserialization does not validate the version.
    pub version: u16,
    /// Human-readable document name with no rendering effect.
    #[serde(default)]
    pub name: Option<String>,
    /// Application-defined JSON properties retained without changing rendering.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
    /// Data sources keyed by the identifiers referenced by layers and terrain.
    #[serde(default)]
    pub sources: HashMap<String, Source>,
    /// Painter order from bottom to top; deserialization assigns zero-based layer indices.
    #[serde(deserialize_with = "layer_order::deserialize_layers")]
    pub layers: Vec<StyleLayer>,
    /// URL template for font glyph ranges.
    #[serde(default)]
    pub glyphs: Option<String>,
    /// Sprite URL or named sprite sources.
    #[serde(default)]
    pub sprite: Option<serde_json::Value>,
    /// Initial `[longitude, latitude]` in degrees, before host camera overrides.
    pub center: Option<[f64; 2]>,
    /// Initial continuous camera zoom, before host overrides.
    pub zoom: Option<f64>,
    /// Initial clockwise rotation from north in degrees.
    pub bearing: Option<f64>,
    /// Initial camera tilt from the map normal in degrees, clamped by the camera pitch limit.
    pub pitch: Option<f64>,
    /// Roll of the view about its axis in degrees, as the GL JS `roll` map option.
    pub roll: Option<f64>,
    /// Map projection and its parameters; omission uses the renderer's default projection.
    #[serde(default)]
    pub projection: Option<ProjectionSpecification>,
    /// Lighting parameters used by style-driven shading.
    #[serde(default)]
    pub light: Option<light::LightSpecification>,
    /// Sky and atmosphere appearance; omission leaves the sky layer disabled.
    #[serde(default)]
    pub sky: Option<sky::SkySpecification>,
    /// Elevation source and vertical exaggeration; omission renders without terrain.
    #[serde(default)]
    pub terrain: Option<terrain::TerrainSpecification>,
    /// Defaults for the values `global-state` expressions read.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub state: HashMap<String, state::StateDeclaration>,
    /// Values set at runtime; they override the defaults and travel with the style to workers.
    #[serde(
        default,
        rename = "globalState",
        skip_serializing_if = "HashMap::is_empty"
    )]
    pub global_state: HashMap<String, serde_json::Value>,
    /// Layers as declared, kept while the layers in use have global state substituted. Public
    /// only so a style can still be built with struct update syntax.
    #[doc(hidden)]
    #[serde(skip)]
    pub state_templates: HashMap<String, state::StateTemplate>,
}

mod default_style;
mod layer_order;
pub mod state;

#[cfg(test)]
mod tests;
