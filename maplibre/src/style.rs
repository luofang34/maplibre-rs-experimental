//! Map style documents, layer properties and source configuration.

#![deny(missing_docs)]

pub use cint::*;
use rustc_hash::FxHashMap as HashMap;
use serde::{Deserialize, Serialize};

pub(crate) mod arabic_shaping;
mod attribution;
pub mod circle;
pub mod diff;
pub mod expression;
pub mod feature_state;
pub use feature_state::{FeatureId, FeatureStateTarget};
pub mod filter;
pub mod heatmap;
pub mod hillshade;
pub mod ids;
pub use ids::{SourceLayerId, StyleLayerId};
pub mod layer;
pub mod light;
pub mod line_gradient;
pub mod line_stroke;
mod opaque_pass;
pub mod property;
pub mod sky;
pub mod source;
pub mod spec;
pub mod symbol;
pub mod terrain;
pub mod transitions;
pub mod translation;
pub mod validation;

use crate::{
    projection::ProjectionSpecification,
    style::{layer::StyleLayer, source::Source},
};

/// Ordered layers, named data sources and initial view settings from a style document.
/// [`Self::validate`] checks layer rendering capabilities, not all style-spec constraints.
#[derive(Serialize, Debug, Clone)]
pub struct Style {
    /// Style specification version, normally 8; deserialization does not validate the version.
    pub version: u16,
    /// Human-readable document name with no rendering effect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Application-defined JSON properties retained without changing rendering.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
    /// Data sources keyed by the identifiers referenced by layers and terrain.
    #[serde(default)]
    pub sources: HashMap<String, Source>,
    /// Painter order from bottom to top; deserialization assigns zero-based layer indices.
    pub layers: Vec<StyleLayer>,
    /// URL template for font glyph ranges.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glyphs: Option<String>,
    /// `@font-face` declarations mapping font families or stacks to TrueType, OpenType, WOFF, or
    /// WOFF2 font URLs.
    #[serde(
        default,
        rename = "font-faces",
        alias = "fontFaces",
        skip_serializing_if = "Option::is_none"
    )]
    pub font_faces: Option<crate::font::FontFaces>,
    /// Font family used to rasterise local CJK ideographs, Kana, and Hangul without requesting
    /// remote `.pbf` glyph ranges (`Some("sans-serif")` by default; `None` when disabled).
    #[serde(
        default = "crate::font::default_local_ideograph_font_family",
        rename = "localIdeographFontFamily",
        alias = "local-ideograph-font-family",
        deserialize_with = "crate::font::deserialize_local_ideograph_font_family",
        skip_serializing_if = "crate::font::is_default_local_ideograph_font_family"
    )]
    pub local_ideograph_font_family: Option<String>,
    /// Sprite URL or named sprite sources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sprite: Option<serde_json::Value>,
    /// Initial `[longitude, latitude]` in degrees, before host camera overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub center: Option<[f64; 2]>,
    /// Initial altitude in metres above sea level of the point the camera looks at, before
    /// host overrides; terrain that holds the center to the ground takes its place.
    #[serde(
        default,
        rename = "centerAltitude",
        skip_serializing_if = "Option::is_none"
    )]
    pub center_altitude: Option<f64>,
    /// Initial continuous camera zoom, before host overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoom: Option<f64>,
    /// Initial clockwise rotation from north in degrees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearing: Option<f64>,
    /// Initial camera tilt from the map normal in degrees, clamped by the camera pitch limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pitch: Option<f64>,
    /// Initial roll in degrees, counterclockwise about the camera's line of sight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roll: Option<f64>,
    /// Map projection and its parameters; omission uses the renderer's default projection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection: Option<ProjectionSpecification>,
    /// Lighting parameters used by style-driven shading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub light: Option<light::LightSpecification>,
    /// Sky and atmosphere appearance; omission leaves the sky layer disabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sky: Option<sky::SkySpecification>,
    /// Elevation source and vertical exaggeration; omission renders without terrain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terrain: Option<terrain::TerrainSpecification>,
    /// Default duration and delay for animated paint property mutations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition: Option<transitions::TransitionSpec>,
    /// In-flight paint property transitions driven by the frame clock.
    #[doc(hidden)]
    #[serde(skip)]
    pub active_transitions: transitions::ActiveTransitions,
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
    /// Per-feature key/value state set at runtime; travels with the style to workers.
    #[serde(
        default,
        rename = "featureState",
        skip_serializing_if = "feature_state::FeatureStateStore::is_empty"
    )]
    pub feature_state: feature_state::FeatureStateStore,
    /// Layers as declared, kept while the layers in use have global state substituted. Public
    /// only so a style can still be built with struct update syntax.
    #[doc(hidden)]
    #[serde(skip)]
    pub state_templates: HashMap<String, state::StateTemplate>,
    /// The light as declared, kept while the light in use has global state substituted.
    #[doc(hidden)]
    #[serde(skip)]
    pub light_template: Option<light::LightSpecification>,
    /// Images a host added by name, which `icon-image` can use next to the sprite's own.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub images: HashMap<String, StyleImage>,
    /// Monotonic revision and lazy worker-snapshot cache; skipped during JSON (de)serialization.
    #[doc(hidden)]
    #[serde(skip)]
    pub revision_state: revision::StyleRevisionState,
}

/// An RGBA image a host added to the style, as GL JS `addImage` takes it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct StyleImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Straight-alpha RGBA bytes, `width * height * 4` of them.
    pub data: Vec<u8>,
    /// Image pixels per layout pixel; 1 unless the image is drawn smaller than it is.
    #[serde(default = "one")]
    pub pixel_ratio: f32,
    /// Whether the alpha channel is a signed distance field that `icon-color` tints.
    #[serde(default)]
    pub sdf: bool,
}

fn one() -> f32 {
    1.0
}

impl Style {
    /// An empty version-8 style without default layers (avoids linking `default_style`).
    pub fn empty() -> Self {
        Self {
            version: 8,
            name: None,
            metadata: Default::default(),
            sources: Default::default(),
            layers: Vec::new(),
            glyphs: None,
            font_faces: None,
            local_ideograph_font_family: crate::font::default_local_ideograph_font_family(),
            sprite: None,
            center: None,
            center_altitude: None,
            zoom: None,
            bearing: None,
            pitch: None,
            roll: None,
            projection: None,
            light: None,
            sky: None,
            terrain: None,
            transition: None,
            active_transitions: Default::default(),
            state: Default::default(),
            global_state: Default::default(),
            feature_state: Default::default(),
            state_templates: Default::default(),
            light_template: None,
            images: Default::default(),
            revision_state: Default::default(),
        }
    }
}

mod default_style;
mod from_value;
mod layer_order;
pub mod mutation;
pub mod pattern_key;
pub(crate) mod revision;
pub mod state;

impl<'de> Deserialize<'de> for Style {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        Self::from_json_value(&value).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests;
