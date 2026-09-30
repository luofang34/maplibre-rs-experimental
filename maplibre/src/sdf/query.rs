//! Queries the screen bounds of symbols accepted by the latest placement pass.
use serde::Serialize;

use crate::{
    coords::WorldTileCoords,
    sdf::SymbolLayersDataComponent,
    style::{
        expression::Value,
        filter::{FeatureContext, Filter, FilterError, GeometryType},
        Style,
    },
    tcs::world::World,
};

#[derive(Default, Debug)]
pub(crate) struct PlacedSymbols(pub(crate) Vec<PlacedSymbol>);

#[derive(Debug)]
pub(crate) struct PlacedSymbol {
    pub(crate) coords: WorldTileCoords,
    pub(crate) layer: String,
    pub(crate) feature: usize,
    pub(crate) rectangles: [Option<[f64; 4]>; 2],
}

/// A rendered symbol and its source attributes, ordered from the topmost style layer.
#[derive(Clone, Debug, Serialize)]
pub struct RenderedSymbol {
    /// Style layer ID.
    pub layer: String,
    /// Style source ID.
    pub source: Option<String>,
    /// Vector source layer.
    pub source_layer: String,
    /// Original feature ID, if supplied by the source.
    pub id: Option<u64>,
    /// Original source attributes.
    pub properties: std::collections::BTreeMap<String, serde_json::Value>,
    /// Rendered text, also useful for accessible selection feedback.
    pub text: String,
    /// Geographic anchor as longitude and latitude in degrees.
    pub coordinates: [f64; 2],
}

/// Region of a query, in screen pixels with y down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum QueryGeometry {
    /// Features under one pixel position.
    Point([f64; 2]),
    /// Features overlapping a rectangle given by two opposite corners.
    Box {
        /// One corner.
        min: [f64; 2],
        /// The opposite corner.
        max: [f64; 2],
    },
}

impl QueryGeometry {
    fn bounds(self) -> Option<[f64; 4]> {
        let [x0, y0, x1, y1] = match self {
            Self::Point([x, y]) => [x, y, x, y],
            Self::Box { min, max } => [min[0], min[1], max[0], max[1]],
        };
        [x0, y0, x1, y1]
            .iter()
            .all(|value| value.is_finite())
            .then(|| [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)])
    }
}

/// Narrows a feature query, as the options of GL JS `queryRenderedFeatures`.
#[derive(Clone, Debug, Default)]
pub struct QueryOptions {
    /// Style layer ids to query; every layer when `None`.
    pub layers: Option<Vec<String>>,
    /// A layer filter, in the legacy or the expression syntax, applied to each candidate.
    pub filter: Option<serde_json::Value>,
}

/// Why a query could not run.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum QueryError {
    /// The geometry has a coordinate that is not a finite number.
    #[error("query geometry must have finite coordinates")]
    InvalidGeometry,
    /// A requested layer is not in the style.
    #[error("layer `{layer}` does not exist in the style and cannot be queried")]
    UnknownLayer {
        /// The requested id.
        layer: String,
    },
    /// The filter is not valid.
    #[error("query filter is invalid")]
    InvalidFilter(#[source] FilterError),
}

/// Returns placed text or icons whose screen bounds contain the point, in pixels with y down.
/// An optional layer list restricts results. Hidden collision candidates are excluded.
pub fn query_rendered_symbols(
    world: &World,
    style: &Style,
    point: [f64; 2],
    layers: Option<&[&str]>,
) -> Vec<RenderedSymbol> {
    let options = QueryOptions {
        layers: layers.map(|layers| layers.iter().map(|layer| (*layer).to_owned()).collect()),
        filter: None,
    };
    query_rendered_symbols_in(world, style, QueryGeometry::Point(point), &options)
        .unwrap_or_default()
}

/// Returns the placed symbols overlapping a point or box, topmost style layer first and, within
/// a layer, by descending sort key. Layers that are hidden are skipped, a filter is evaluated
/// with each symbol's properties at its tile's zoom, and unloaded tiles yield nothing. Only
/// symbols are queryable; fill, line and circle features are not retained for queries.
pub fn query_rendered_symbols_in(
    world: &World,
    style: &Style,
    geometry: QueryGeometry,
    options: &QueryOptions,
) -> Result<Vec<RenderedSymbol>, QueryError> {
    let bounds = geometry.bounds().ok_or(QueryError::InvalidGeometry)?;
    for layer in options.layers.iter().flatten() {
        if !style.layers.iter().any(|candidate| &candidate.id == layer) {
            return Err(QueryError::UnknownLayer {
                layer: layer.clone(),
            });
        }
    }
    let filter = options
        .filter
        .as_ref()
        .map(Filter::parse)
        .transpose()
        .map_err(QueryError::InvalidFilter)?;
    let Some(placed) = world.resources.get::<PlacedSymbols>() else {
        return Ok(Vec::new());
    };
    let mut matches = Vec::new();
    for hit in &placed.0 {
        if options
            .layers
            .as_ref()
            .is_some_and(|layers| !layers.contains(&hit.layer))
            || !hit.rectangles.iter().flatten().any(|r| {
                r[0] <= bounds[2] && r[2] >= bounds[0] && r[1] <= bounds[3] && r[3] >= bounds[1]
            })
        {
            continue;
        }
        let Some(layer) = world
            .tiles
            .query::<&SymbolLayersDataComponent>(hit.coords)
            .and_then(|component| {
                component
                    .layers
                    .iter()
                    .find(|layer| layer.style_layer_id == hit.layer)
            })
        else {
            continue;
        };
        let Some(feature) = layer.features.get(hit.feature) else {
            continue;
        };
        let Some(style_layer) = style.layers.iter().find(|layer| layer.id == hit.layer) else {
            continue;
        };
        if style_layer.is_hidden() {
            continue;
        }
        if let Some(filter) = &filter {
            let id = feature
                .data
                .id
                .map(|id| Value::from_json(&serde_json::json!(id)));
            let passes = filter.evaluate(&FeatureContext {
                properties: &feature.data.properties,
                geometry_type: GeometryType::Point,
                id,
                zoom: f64::from(u8::from(hit.coords.z)),
            });
            if !passes {
                continue;
            }
        }
        let scale = 2_f64.powi(i32::from(u8::from(hit.coords.z)));
        let x = (f64::from(hit.coords.x) + f64::from(feature.text_anchor.x) / 4096.0) / scale;
        let y = (f64::from(hit.coords.y) + f64::from(feature.text_anchor.y) / 4096.0) / scale;
        matches.push((
            style_layer.index,
            feature.data.sort_key,
            RenderedSymbol {
                layer: hit.layer.clone(),
                source: style_layer.source.clone(),
                source_layer: layer.source_layer.clone(),
                id: feature.data.id,
                properties: feature
                    .data
                    .properties
                    .iter()
                    .map(|(key, value)| (key.clone(), value.to_json()))
                    .collect(),
                text: feature.str.clone(),
                coordinates: [
                    (x * 360.0 + 180.0).rem_euclid(360.0) - 180.0,
                    (std::f64::consts::PI * (1.0 - 2.0 * y))
                        .sinh()
                        .atan()
                        .to_degrees(),
                ],
            },
        ));
    }
    matches.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.total_cmp(&a.1)));
    Ok(matches.into_iter().map(|(_, _, feature)| feature).collect())
}

#[cfg(test)]
mod tests;
