//! Features under a screen point or box, as GL JS `queryRenderedFeatures`.
//!
//! Symbols come from the latest placement pass. Fill and line features come from the geometry
//! index each worker builds for a tile; style layers are resolved against the live style at query
//! time, so filters, visibility, paint edits and layer removal never leave the index stale.

use std::{
    cmp::Reverse,
    collections::{BTreeMap, HashSet},
};

use serde::Serialize;

use crate::{
    io::geometry_index::{ExactGeometry, IndexedGeometry},
    render::view_state::ViewState,
    sdf::query::{
        query_rendered_symbols_in, QueryError, QueryGeometry, QueryOptions, RenderedSymbol,
    },
    style::{
        expression::Value,
        filter::{FeatureContext, Filter, GeometryType},
        layer::{LayerPaint, LinePaint, StyleLayer},
        source::GEOJSON_LAYER,
        translation::layer_translate_pixels,
        Style,
    },
    tcs::world::World,
};

mod extrusion;
mod ground;
mod tiles;

use crate::coords::Zoom;
use tiles::{
    camera_query_bounds, candidates_in, ground_corners, ground_region, tiles_in, QueryTile,
};

/// Deepest tile zoom a query looks at.
const MAX_QUERY_ZOOM: i32 = 22;

/// A feature drawn by a style layer, in the shape of GL JS `MapGeoJSONFeature`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct QueriedFeature {
    /// Style layer id.
    pub layer: String,
    /// Style source id.
    pub source: Option<String>,
    /// Layer inside the source.
    pub source_layer: String,
    /// The feature's id, when the source assigned a whole-number one.
    pub id: Option<u64>,
    /// The feature's attributes.
    pub properties: BTreeMap<String, serde_json::Value>,
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
            id: symbol.id,
            properties: symbol.properties,
            geometry_type: symbol.geometry_type.name(),
            text: Some(symbol.text),
        }
    }
}

/// A feature's properties as its source gives them, without the feature state an expression
/// reads through them.
pub(crate) fn source_properties(
    properties: &crate::style::expression::FeatureProperties,
) -> BTreeMap<String, serde_json::Value> {
    properties
        .iter()
        .filter(|(key, _)| !key.starts_with(crate::style::expression::FEATURE_STATE_PREFIX))
        .map(|(key, value)| (key.clone(), value.to_json()))
        .collect()
}

/// Features under `geometry`, topmost style layer first.
///
/// Symbols use their placement; fill and line features are located on the ground plane of a flat
/// view and need the geometry index, which only native workers send. A fill or line query in a
/// view with terrain or an active globe is refused rather than answered wrongly, and a point in
/// the sky returns nothing.
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
            (index, None, QueriedFeature::from(symbol))
        })
        .collect();
    found.extend(vector_features(
        world, style, view_state, geometry, options,
    )?);
    Ok(in_drawing_order(found))
}

/// Features topmost first, as GL JS `Style.queryRenderedFeatures` orders them: layer by layer
/// from the top, where each fill extrusion layer gives out the nearest remaining extrusions
/// for as long as the nearest is in that layer or above it.
fn in_drawing_order(found: Vec<(u32, Option<f64>, QueriedFeature)>) -> Vec<QueriedFeature> {
    let (mut extruded, flat): (Vec<_>, Vec<_>) =
        found.into_iter().partition(|(_, depth, _)| depth.is_some());
    // Nearest last, so the nearest is popped first.
    extruded.sort_by(|a, b| b.1.unwrap_or(0.0).total_cmp(&a.1.unwrap_or(0.0)));
    let extrusion_layers: HashSet<u32> = extruded.iter().map(|(layer, _, _)| *layer).collect();
    let mut layers: Vec<u32> = flat
        .iter()
        .map(|(layer, _, _)| *layer)
        .chain(extrusion_layers.iter().copied())
        .collect();
    layers.sort_unstable_by_key(|layer| Reverse(*layer));
    layers.dedup();
    let mut flat_by_layer: BTreeMap<u32, Vec<QueriedFeature>> = BTreeMap::new();
    for (layer, _, feature) in flat {
        flat_by_layer.entry(layer).or_default().push(feature);
    }
    let mut ordered = Vec::new();
    for layer in layers {
        if extrusion_layers.contains(&layer) {
            while extruded
                .last()
                .is_some_and(|(nearest, _, _)| *nearest >= layer)
            {
                if let Some((_, _, feature)) = extruded.pop() {
                    ordered.push(feature);
                }
            }
        }
        if let Some(features) = flat_by_layer.remove(&layer) {
            ordered.extend(features);
        }
    }
    ordered
}

/// A layer a fill or line feature can be reported for, with its filter parsed once.
struct Candidate<'a> {
    layer: &'a StyleLayer,
    filter: Option<Filter>,
    /// How far from a line, in screen pixels, its features can be drawn and still be met: half
    /// its width and its offset.
    reach_pixels: f64,
    /// Where the layer draws its features from where they are, in screen pixels along the
    /// map's axes: its `*-translate`.
    translate: [f64; 2],
}

impl<'a> Candidate<'a> {
    /// A layer whose filter cannot be parsed draws nothing, so it also matches nothing.
    fn new(layer: &'a StyleLayer, zoom: f64, bearing: f64) -> Option<Self> {
        let filter = match &layer.filter {
            Some(filter) => Some(Filter::parse(filter).ok()?),
            None => None,
        };
        let reach_pixels = match &layer.paint {
            Some(LayerPaint::Line(paint)) => line_margin_pixels(paint, zoom),
            _ => 0.0,
        };
        Some(Self {
            layer,
            filter,
            reach_pixels,
            translate: layer_translate_pixels(layer.paint.as_ref(), zoom, bearing),
        })
    }

    /// How far from the query, in screen pixels, a feature the layer draws there can lie.
    fn margin_pixels(&self) -> f64 {
        self.reach_pixels + self.translate[0].hypot(self.translate[1])
    }
}

/// The farthest a line layer reaches from its features: half its width and its offset, at their
/// widest where they vary by feature.
fn line_margin_pixels(paint: &LinePaint, zoom: f64) -> f64 {
    // A width or offset set by feature is not known before the feature is; this much covers
    // what styles give them.
    const BY_FEATURE: f64 = 64.0;
    let [half_width, offset] = ground::line_reach(paint, &Default::default(), zoom);
    let varies = [&paint.line_width, &paint.line_gap_width, &paint.line_offset]
        .into_iter()
        .flatten()
        .any(|property| !property.is_feature_constant());
    half_width + offset.abs() + if varies { BY_FEATURE } else { 0.0 }
}

/// What one query keeps while it walks tiles.
struct Search<'a> {
    candidates: Vec<Candidate<'a>>,
    query_filter: Option<Filter>,
    seen: HashSet<(String, Identity)>,
    /// Each feature with its layer's index and, for a fill extrusion, its depth on screen.
    found: Vec<(u32, Option<f64>, QueriedFeature)>,
    /// The query in window pixels, which extrusions are met against as they stand.
    screen: extrusion::ScreenQuery,
    /// The corners of the ground the query covers, in world pixels.
    ground: Vec<[f64; 2]>,
    view_state: &'a ViewState,
}

/// Fill and line features are located on a flat ground plane.
fn refuse_unsupported_view(style: &Style, zoom: f64) -> Result<(), QueryError> {
    if style.terrain.is_some() {
        return Err(QueryError::UnsupportedView { reason: "terrain" });
    }
    if style
        .projection
        .as_ref()
        .is_some_and(|projection| projection.projection_type.globe_transition(zoom) != 0.0)
    {
        return Err(QueryError::UnsupportedView {
            reason: "an active globe",
        });
    }
    Ok(())
}

fn vector_features(
    world: &World,
    style: &Style,
    view_state: &ViewState,
    geometry: QueryGeometry,
    options: &QueryOptions,
) -> Result<Vec<(u32, Option<f64>, QueriedFeature)>, QueryError> {
    let zoom = view_state.zoom();
    let bearing = view_state.camera().get_bearing().0;
    let candidates: Vec<Candidate> = style
        .layers
        .iter()
        .filter(|layer| {
            matches!(layer.type_.as_str(), "fill" | "line" | "fill-extrusion")
                && layer.is_visible_at(zoom.value())
                && options
                    .layers
                    .as_ref()
                    .is_none_or(|layers| layers.contains(&layer.id))
        })
        .filter_map(|layer| Candidate::new(layer, zoom.value(), bearing))
        .collect();
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    refuse_unsupported_view(style, zoom.value())?;
    let query_filter = options
        .filter
        .as_ref()
        .map(Filter::parse)
        .transpose()
        .map_err(QueryError::InvalidFilter)?;
    let Some(bounds) = geometry.bounds() else {
        return Err(QueryError::InvalidGeometry);
    };
    // An extrusion stands up from the ground, so it can meet the query far from where the
    // query meets the ground: every tile in view is searched.
    let extrudes = candidates
        .iter()
        .any(|candidate| candidate.layer.type_ == "fill-extrusion");
    let margin = candidates
        .iter()
        .map(Candidate::margin_pixels)
        .fold(0.0, f64::max);
    let searched = if extrudes {
        camera_query_bounds(view_state, bounds)
    } else {
        bounds
    };
    let Some(region) = ground_region(view_state, searched) else {
        return Ok(Vec::new());
    };
    let screen = match geometry {
        QueryGeometry::Point(point) => extrusion::ScreenQuery::Point(point),
        QueryGeometry::Box { .. } => extrusion::ScreenQuery::Box(bounds),
    };
    let mut search = Search {
        candidates,
        query_filter,
        seen: HashSet::new(),
        found: Vec::new(),
        screen,
        ground: ground_corners(view_state, bounds),
        view_state,
    };
    search_tiles(world, (region, zoom, margin), &mut search);
    Ok(search.found)
}

/// Collects from the finest level of tiles that holds any index for the region, looking
/// `margin` screen pixels beyond the query for features a layer draws within reach of it.
fn search_tiles(world: &World, (region, zoom, margin): ([f64; 4], Zoom, f64), search: &mut Search) {
    // One level above the view zoom covers sources whose tiles are finer than the view's.
    let top = (zoom.value().floor() as i32 + 1).clamp(0, MAX_QUERY_ZOOM);
    for z in (0..=top).rev() {
        let mut any = false;
        for mut tile in tiles_in(region, margin, zoom, z as u8) {
            let Some(indexes) = world.tiles.geometry_index.tile_indexes(&tile.coords) else {
                continue;
            };
            any = true;
            tile.footprint = tile.footprint_of(&search.ground);
            let tile = &tile;
            let widen = margin * tile.units_per_pixel;
            let [x0, y0, x1, y1] = tile.local;
            let window = [x0 - widen, y0 - widen, x1 + widen, y1 + widen];
            for (source, index) in indexes {
                for geometry in candidates_in(index, window) {
                    collect(geometry, source, tile, search);
                }
            }
        }
        if any {
            break;
        }
    }
}

/// Whether a fill or line feature meets the query in this tile.
fn flat_hit(
    geometry: &IndexedGeometry<f64>,
    tile: &QueryTile,
    candidate: &Candidate,
    zoom: f64,
) -> bool {
    let Some(footprint) = &tile.footprint else {
        return false;
    };
    let line = match &candidate.layer.paint {
        Some(LayerPaint::Line(paint)) => ground::line_reach(paint, &geometry.properties, zoom)
            .map(|pixels| pixels * tile.units_per_pixel),
        _ => [0.0; 2],
    };
    let translate = candidate
        .translate
        .map(|pixels| pixels * tile.units_per_pixel);
    ground::touches(
        geometry,
        footprint,
        (candidate.layer.type_.as_str(), translate),
        line,
    )
}

fn collect(
    geometry: &IndexedGeometry<f64>,
    source: Option<&str>,
    tile: &QueryTile,
    search: &mut Search,
) {
    let (kind, geometry_type) = match &geometry.exact {
        ExactGeometry::Polygon(_) => (GeometryType::Polygon, "Polygon"),
        ExactGeometry::LineString(_) => (GeometryType::LineString, "LineString"),
    };
    let id = geometry.id.map(|id| Value::Number(id as f64));
    let context = FeatureContext {
        properties: &geometry.properties,
        geometry_type: kind,
        id,
        zoom: f64::from(tile.zoom_level),
    };
    let Search {
        candidates,
        query_filter,
        seen,
        found,
        screen,
        view_state,
        ..
    } = search;
    for candidate in candidates.iter() {
        let layer = candidate.layer;
        if layer.source.as_deref() != source
            || layer.source_layer.as_deref().unwrap_or(GEOJSON_LAYER) != &*geometry.source_layer
            || candidate
                .filter
                .as_ref()
                .is_some_and(|filter| !filter.evaluate(&context))
            || query_filter
                .as_ref()
                .is_some_and(|filter| !filter.evaluate(&context))
        {
            continue;
        }
        let depth = match (&geometry.exact, candidate.layer.type_.as_str()) {
            (ExactGeometry::Polygon(polygon), "fill-extrusion") => {
                match extrusion::extruded_depth(
                    polygon,
                    tile,
                    candidate,
                    (&geometry.properties, *screen, view_state),
                ) {
                    Some(depth) => Some(depth),
                    None => continue,
                }
            }
            _ if flat_hit(geometry, tile, candidate, view_state.zoom().value()) => None,
            _ => continue,
        };
        let identity = match geometry.id {
            Some(id) => Identity::Id(id),
            None => Identity::Part(std::sync::Arc::as_ptr(&geometry.properties) as usize),
        };
        if !seen.insert((layer.id.clone(), identity)) {
            continue;
        }
        found.push((
            layer.index,
            depth,
            QueriedFeature {
                layer: layer.id.clone(),
                source: layer.source.clone(),
                source_layer: geometry.source_layer.to_string(),
                id: geometry.id,
                properties: source_properties(&geometry.properties),
                geometry_type,
                text: None,
            },
        ));
    }
}

/// What makes two parts one feature: their shared id, or the property map they share.
#[derive(PartialEq, Eq, Hash)]
enum Identity {
    Id(u64),
    Part(usize),
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
