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
    style::Style,
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

pub use crate::style::feature_state::QueriedFeatureId;

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
    crate::tcs::geometry_index::register_mvt_materializer(
        crate::io::vector_tile::index_mvt_bytes_rescaled,
    );
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
