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

/// The map's bearing when the symbols were placed, in radians, which decides the order of
/// labels drawn by their height on the screen.
#[derive(Default, Debug, Clone, Copy)]
pub(crate) struct PlacementBearing(pub(crate) f64);

#[derive(Clone, Debug)]
pub(crate) struct PlacedSymbol {
    pub(crate) coords: WorldTileCoords,
    pub(crate) layer: String,
    pub(crate) feature: usize,
    pub(crate) rectangles: [Option<[f64; 4]>; 2],
    /// One box per glyph of a text that follows a line; empty for other labels, whose text
    /// is the first rectangle.
    pub(crate) glyph_boxes: Vec<[f64; 4]>,
}

impl PlacedSymbol {
    /// The screen areas a pointer can hit: the icon, and the text, glyph by glyph on a line.
    fn hit_areas(&self) -> impl Iterator<Item = [f64; 4]> + '_ {
        let text = if self.glyph_boxes.is_empty() {
            self.rectangles[0]
        } else {
            None
        };
        text.into_iter()
            .chain(self.rectangles[1])
            .chain(self.glyph_boxes.iter().copied())
    }
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
    /// The kind of the source feature's geometry, which GL JS reports for a label.
    pub geometry_type: GeometryType,
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
    pub(crate) fn bounds(self) -> Option<[f64; 4]> {
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
    /// Fill and line features cannot be located in this view.
    #[error("fill and line features cannot be queried with {reason}")]
    UnsupportedView {
        /// What the view uses that the query cannot unproject through.
        reason: &'static str,
    },
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
/// symbols are queryable here; use [`crate::query::query_rendered_features`] for fill and line features.
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
    let bearing = world
        .resources
        .get::<PlacementBearing>()
        .map_or(0.0, |bearing| bearing.0);
    let mut matches = Vec::new();
    for hit in &placed.0 {
        if options
            .layers
            .as_ref()
            .is_some_and(|layers| !layers.contains(&hit.layer))
            || !hit.hit_areas().any(|r| {
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
                geometry_type: feature.data.geometry_type,
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
        let zoom = f64::from(u8::from(hit.coords.z));
        let draw_order = match &style_layer.paint {
            Some(crate::style::layer::LayerPaint::Symbol(paint))
                if crate::sdf::tessellation::sorts_by_height(paint, zoom) =>
            {
                // As GL JS `sortFeatures`: by height on the rotated screen, from anchors in
                // whole units of its 8192-unit tiles, and equal heights the later feature first.
                let angle = -bearing;
                let [x, y] = [feature.text_anchor.x, feature.text_anchor.y]
                    .map(|value| (f64::from(value) * 2.0).round());
                let height = (angle.sin() * x + angle.cos() * y).round() as i64;
                (height, -(hit.feature as i64))
            }
            _ => (0, hit.feature as i64),
        };
        matches.push((
            style_layer.index,
            (u8::from(hit.coords.z), hit.coords.y, hit.coords.x),
            draw_order,
            RenderedSymbol {
                layer: hit.layer.clone(),
                source: style_layer.source.clone(),
                source_layer: layer.source_layer.clone(),
                id: feature.data.id,
                properties: crate::query::source_properties(&feature.data.properties),
                geometry_type: feature.data.geometry_type,
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
    // As GL JS `queryRenderedSymbols`: the topmost layer first, its tiles from the lowest zoom
    // and then top to bottom, and in each tile the labels drawn last first. A tile's labels are
    // drawn by symbol-sort-key, or by height on the rotated screen where they may overlap.
    matches.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(b.2.cmp(&a.2)));
    Ok(matches
        .into_iter()
        .map(|(_, _, _, feature)| feature)
        .collect())
}

#[cfg(test)]
mod tests;
