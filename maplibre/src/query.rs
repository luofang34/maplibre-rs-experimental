//! Features under a screen point or box, as GL JS `queryRenderedFeatures`.
//!
//! Symbols come from the latest placement pass. Fill, line, circle, heatmap and extrusion features
//! come from the geometry index each worker builds for a tile and are evaluated against the live
//! style across flat Mercator, 3D terrain, and 3D globe / transition projections.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::{
    render::view_state::ViewState,
    sdf::query::{
        query_rendered_symbols_in, QueryError, QueryGeometry, QueryOptions, RenderedSymbol,
    },
    style::{expression::Value, Style},
    tcs::world::World,
};

mod circle;
mod collect;
mod extrusion;
mod globe;
mod ground;
mod order;
mod overzoomed;
mod surface;
mod tiles;

use order::{in_drawing_order, Candidate};

/// A feature identifier returned by [`query_rendered_features`] or source queries: a non-negative
/// integer or a string (such as a string `promoteId` or GeoJSON string `id`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum QueriedFeatureId {
    /// Whole-number feature identifier.
    Number(u64),
    /// String feature identifier.
    String(String),
}

impl QueriedFeatureId {
    /// Parses a JSON feature id or promoted property value.
    pub fn from_json(value: &serde_json::Value) -> Option<Self> {
        if let Some(n) = value.as_u64() {
            return Some(Self::Number(n));
        }
        if let Some(n) = value.as_f64() {
            if n >= 0.0 && n.fract() == 0.0 && n < 9.0e15 {
                return Some(Self::Number(n as u64));
            }
            if n.is_finite() {
                return Some(Self::String(crate::style::expression::js_number(n)));
            }
        }
        value.as_str().map(|s| Self::String(s.to_owned()))
    }

    /// Parses an evaluated expression value into a feature identifier.
    pub(crate) fn from_expr_value(value: &Value) -> Option<Self> {
        match value {
            Value::Number(n) if *n >= 0.0 && n.fract() == 0.0 && *n < 9.0e15 => {
                Some(Self::Number(*n as u64))
            }
            Value::Number(n) if n.is_finite() => {
                Some(Self::String(crate::style::expression::js_number(*n)))
            }
            Value::String(s) => Some(Self::String(s.clone())),
            _ => None,
        }
    }

    /// Converts this identifier into an expression [`Value`] for `["id"]` / `$id` filters.
    pub(crate) fn to_expr_value(&self) -> Value {
        match self {
            Self::Number(n) => Value::Number(*n as f64),
            Self::String(s) => Value::String(s.clone()),
        }
    }
}

impl From<u64> for QueriedFeatureId {
    fn from(value: u64) -> Self {
        Self::Number(value)
    }
}

impl From<String> for QueriedFeatureId {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for QueriedFeatureId {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

impl PartialEq<u64> for QueriedFeatureId {
    fn eq(&self, other: &u64) -> bool {
        matches!(self, Self::Number(n) if n == other)
    }
}

impl PartialEq<&str> for QueriedFeatureId {
    fn eq(&self, other: &&str) -> bool {
        matches!(self, Self::String(s) if s == other)
    }
}

/// Resolves the effective expression `["id"]` / `$id` value for a feature, respecting `promoteId`.
/// When `promote_property` is `Some`, it overrides `raw_id` without falling back to it.
pub(crate) fn resolve_effective_expr_id(
    raw_id: Option<u64>,
    properties: &crate::style::expression::FeatureProperties,
    promote_property: Option<&str>,
) -> Option<Value> {
    if let Some(prop) = promote_property {
        return match properties.get(prop) {
            Some(Value::String(s)) => Some(Value::String(s.clone())),
            Some(Value::Number(n)) if n.is_finite() => Some(Value::Number(*n)),
            _ => None,
        };
    }
    raw_id.map(|id| Value::Number(id as f64))
}

fn source_promote_property<'a>(
    style: &'a Style,
    source: Option<&str>,
    source_layer: &str,
) -> Option<&'a str> {
    let promote = match style.sources.get(source?)? {
        crate::style::source::Source::Vector(v) => v.promote_id.as_ref()?,
        crate::style::source::Source::GeoJson(g) => g.promote_id.as_ref()?,
        _ => return None,
    };
    promote.property_for(Some(source_layer))
}

/// A feature drawn by a style layer, in the shape of GL JS `MapGeoJSONFeature`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct QueriedFeature {
    /// Style layer id.
    pub layer: String,
    /// Style source id.
    pub source: Option<String>,
    /// Layer inside the source.
    pub source_layer: String,
    /// The feature's id (numeric or string, respecting `promoteId`).
    pub id: Option<QueriedFeatureId>,
    /// The feature's attributes.
    pub properties: BTreeMap<String, serde_json::Value>,
    /// Runtime `feature-state` entries currently assigned to this feature.
    pub state: BTreeMap<String, serde_json::Value>,
    /// `Point` for a symbol, `Polygon` for a fill and `LineString` for a line.
    pub geometry_type: &'static str,
    /// The rendered text of a symbol.
    pub text: Option<String>,
}

impl From<RenderedSymbol> for QueriedFeature {
    fn from(symbol: RenderedSymbol) -> Self {
        Self {
            layer: symbol.layer,
            source: symbol.source,
            source_layer: symbol.source_layer,
            id: symbol.id.map(QueriedFeatureId::Number),
            properties: symbol.properties,
            state: symbol.state,
            geometry_type: symbol.geometry_type.name(),
            text: Some(symbol.text),
        }
    }
}

pub(crate) use crate::style::feature_state::extract_source_properties as source_properties;

/// Features under `geometry`, topmost style layer first, across flat Mercator, 3D terrain, and
/// 3D globe / Mercator-to-globe transition views.
///
/// Symbols use their screen placement; vector layers use the per-tile [`crate::io::geometry_index::GeometryIndex`]
/// and resolve surface intersections through [`ResolvedFrameView`]. A point in the sky or beyond
/// the globe limb returns nothing.
pub fn query_rendered_features(
    world: &World,
    style: &Style,
    view_state: &ViewState,
    geometry: QueryGeometry,
    options: &QueryOptions,
) -> Result<Vec<QueriedFeature>, QueryError> {
    let symbols = query_rendered_symbols_in(world, style, geometry, options)?;
    let mut found: Vec<(u32, Option<f64>, QueriedFeature)> = symbols
        .into_iter()
        .map(|symbol| {
            let index = style
                .layers
                .iter()
                .find(|layer| layer.id == symbol.layer)
                .map_or(0, |layer| layer.index);
            let mut feature = QueriedFeature::from(symbol);
            if let Some(prop) =
                source_promote_property(style, feature.source.as_deref(), &feature.source_layer)
            {
                feature.id = feature
                    .properties
                    .get(prop)
                    .and_then(QueriedFeatureId::from_json);
            }
            (index, None, feature)
        })
        .collect();
    found.extend(collect::vector_features(
        world, style, view_state, geometry, options,
    )?);
    Ok(in_drawing_order(found))
}

impl crate::context::MapContext {
    /// Features under a point or box, topmost first; see [`query_rendered_features`].
    pub fn query_rendered_features(
        &self,
        geometry: QueryGeometry,
        options: &QueryOptions,
    ) -> Result<Vec<QueriedFeature>, QueryError> {
        query_rendered_features(
            &self.world,
            &self.style,
            &self.view_state,
            geometry,
            options,
        )
    }
}

#[cfg(test)]
mod tests;
