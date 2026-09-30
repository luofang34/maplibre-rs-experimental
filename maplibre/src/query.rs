//! Features under a screen point or box, as GL JS `queryRenderedFeatures`.
//!
//! Symbols come from the latest placement pass. Fill and line features come from the geometry
//! index each worker builds for a tile; style layers are resolved against the live style at query
//! time, so filters, visibility, paint edits and layer removal never leave the index stale.

use std::{
    cmp::Reverse,
    collections::{BTreeMap, HashSet},
};

use cgmath::Vector2;
use geo::prelude::*;
use geo_types::{Coord, Rect};
use serde::Serialize;

use crate::{
    coords::{WorldTileCoords, Zoom, ZoomLevel, EXTENT, TILE_SIZE},
    io::geometry_index::{ExactGeometry, IndexedGeometry, TileIndex},
    render::view_state::ViewState,
    sdf::query::{
        query_rendered_symbols_in, QueryError, QueryGeometry, QueryOptions, RenderedSymbol,
    },
    style::{
        expression::Value,
        filter::{FeatureContext, Filter, GeometryType},
        layer::{LayerPaint, StyleLayer},
        source::GEOJSON_LAYER,
        Style,
    },
    tcs::world::World,
};

/// Tiles a single query reads at one zoom before it uses a coarser one.
const MAX_TILES: i64 = 256;
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
            geometry_type: "Point",
            text: Some(symbol.text),
        }
    }
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
    let mut found: Vec<(u32, QueriedFeature)> = symbols
        .into_iter()
        .map(|symbol| {
            let index = style
                .layers
                .iter()
                .find(|layer| layer.id == symbol.layer)
                .map_or(0, |layer| layer.index);
            (index, QueriedFeature::from(symbol))
        })
        .collect();
    found.extend(vector_features(
        world, style, view_state, geometry, options,
    )?);
    found.sort_by_key(|(index, _)| Reverse(*index));
    Ok(found.into_iter().map(|(_, feature)| feature).collect())
}

/// A layer a fill or line feature can be reported for, with its filter parsed once.
struct Candidate<'a> {
    layer: &'a StyleLayer,
    filter: Option<Filter>,
    /// How far from a line, in screen pixels, still counts as touching it: half its width.
    reach_pixels: f64,
}

impl<'a> Candidate<'a> {
    /// A layer whose filter cannot be parsed draws nothing, so it also matches nothing.
    fn new(layer: &'a StyleLayer, zoom: f64) -> Option<Self> {
        let filter = match &layer.filter {
            Some(filter) => Some(Filter::parse(filter).ok()?),
            None => None,
        };
        let reach_pixels = match &layer.paint {
            Some(LayerPaint::Line(paint)) => {
                paint
                    .line_width
                    .as_ref()
                    .and_then(|width| width.evaluate_at_zoom(zoom))
                    .unwrap_or(1.0)
                    / 2.0
            }
            _ => 0.0,
        };
        Some(Self {
            layer,
            filter,
            reach_pixels: f64::from(reach_pixels),
        })
    }
}

/// What one query keeps while it walks tiles.
struct Search<'a> {
    candidates: Vec<Candidate<'a>>,
    query_filter: Option<Filter>,
    seen: HashSet<(String, Identity)>,
    found: Vec<(u32, QueriedFeature)>,
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
) -> Result<Vec<(u32, QueriedFeature)>, QueryError> {
    let zoom = view_state.zoom();
    let candidates: Vec<Candidate> = style
        .layers
        .iter()
        .filter(|layer| {
            matches!(layer.type_.as_str(), "fill" | "line")
                && layer.is_visible_at(zoom.value())
                && options
                    .layers
                    .as_ref()
                    .is_none_or(|layers| layers.contains(&layer.id))
        })
        .filter_map(|layer| Candidate::new(layer, zoom.value()))
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
    let Some(region) = ground_region(view_state, bounds) else {
        return Ok(Vec::new());
    };
    let mut search = Search {
        candidates,
        query_filter,
        seen: HashSet::new(),
        found: Vec::new(),
    };
    // One level above the view zoom covers sources whose tiles are finer than the view's.
    let top = (zoom.value().floor() as i32 + 1).clamp(0, MAX_QUERY_ZOOM);
    for z in (0..=top).rev() {
        let tiles = tiles_in(region, zoom, z as u8);
        if tiles.is_empty() {
            continue;
        }
        let mut any = false;
        for tile in &tiles {
            let Some(indexes) = world.tiles.geometry_index.tile_indexes(&tile.coords) else {
                continue;
            };
            any = true;
            for (source, index) in indexes {
                for geometry in candidates_in(index, tile.local) {
                    collect(geometry, source, tile, &mut search);
                }
            }
        }
        if any {
            break;
        }
    }
    Ok(search.found)
}

/// The rectangle of world pixels a screen box covers on the ground plane, or `None` when none of
/// its corners reaches the ground (it is all sky).
fn ground_region(view_state: &ViewState, bounds: [f64; 4]) -> Option<[f64; 4]> {
    let inverted = view_state.inverted_view_projection().ok()?;
    let mut region: Option<[f64; 4]> = None;
    for (x, y) in [
        (bounds[0], bounds[1]),
        (bounds[2], bounds[1]),
        (bounds[2], bounds[3]),
        (bounds[0], bounds[3]),
    ] {
        let Some(point) =
            view_state.window_to_world_at_ground(&Vector2::new(x, y), &inverted, true)
        else {
            continue;
        };
        if !point.x.is_finite() || !point.y.is_finite() {
            continue;
        }
        region = Some(match region {
            None => [point.x, point.y, point.x, point.y],
            Some(r) => [
                r[0].min(point.x),
                r[1].min(point.y),
                r[2].max(point.x),
                r[3].max(point.y),
            ],
        });
    }
    region
}

/// One tile a query touches: its canonical coordinates and the query rectangle in its grid.
struct QueryTile {
    coords: WorldTileCoords,
    /// Query rectangle in tile units, as `[min x, min y, max x, max y]`.
    local: [f64; 4],
    /// Tile units that make one screen pixel.
    units_per_pixel: f64,
    zoom_level: u8,
}

/// The tiles at grid level `z` that a region of world pixels at view zoom `zoom` covers.
fn tiles_in(region: [f64; 4], zoom: Zoom, z: u8) -> Vec<QueryTile> {
    let scale = zoom.scale_to_zoom_level(ZoomLevel::new(z));
    let to_grid = |world: f64| world / TILE_SIZE * scale;
    let (x0, y0) = (to_grid(region[0]), to_grid(region[1]));
    let (x1, y1) = (to_grid(region[2]), to_grid(region[3]));
    let (tx0, tx1) = (x0.floor() as i64, x1.floor() as i64);
    let (ty0, ty1) = (y0.floor() as i64, y1.floor() as i64);
    let tiles_wide = 1_i64 << z;
    if (tx1 - tx0 + 1) * (ty1 - ty0 + 1) > MAX_TILES {
        return Vec::new();
    }
    let units_per_pixel = EXTENT * scale / TILE_SIZE;
    let mut tiles = Vec::new();
    for ty in ty0.max(0)..=ty1.min(tiles_wide - 1) {
        for tx in tx0..=tx1 {
            let canonical = tx.rem_euclid(tiles_wide);
            tiles.push(QueryTile {
                coords: WorldTileCoords {
                    x: canonical as i32,
                    y: ty as i32,
                    z: ZoomLevel::new(z),
                },
                local: [
                    (x0 - tx as f64) * EXTENT,
                    (y0 - ty as f64) * EXTENT,
                    (x1 - tx as f64) * EXTENT,
                    (y1 - ty as f64) * EXTENT,
                ],
                units_per_pixel,
                zoom_level: z,
            });
        }
    }
    tiles
}

/// The geometries whose bounds meet the query rectangle.
fn candidates_in(index: &TileIndex, local: [f64; 4]) -> Vec<&IndexedGeometry<f64>> {
    let window = rstar::AABB::from_corners(
        geo_types::Point::new(local[0], local[1]),
        geo_types::Point::new(local[2], local[3]),
    );
    match index {
        TileIndex::Spatial { tree } => tree.locate_in_envelope_intersecting(&window).collect(),
        TileIndex::Linear { list } => list
            .iter()
            .filter(|geometry| {
                use rstar::Envelope;
                geometry.bounds.intersects(&window)
            })
            .collect(),
    }
}

fn touches(geometry: &IndexedGeometry<f64>, tile: &QueryTile, candidate: &Candidate) -> bool {
    let [x0, y0, x1, y1] = tile.local;
    let reach = candidate.reach_pixels * tile.units_per_pixel;
    let area = Rect::new(
        Coord {
            x: x0 - reach,
            y: y0 - reach,
        },
        Coord {
            x: x1 + reach,
            y: y1 + reach,
        },
    );
    match (&geometry.exact, candidate.layer.type_.as_str()) {
        (ExactGeometry::Polygon(polygon), "fill") => polygon.intersects(&area),
        (ExactGeometry::Polygon(polygon), "line") => {
            polygon.exterior().intersects(&area)
                || polygon
                    .interiors()
                    .iter()
                    .any(|ring| ring.intersects(&area))
        }
        (ExactGeometry::LineString(line), "line") => line.intersects(&area),
        _ => false,
    }
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
            || !touches(geometry, tile, candidate)
        {
            continue;
        }
        let identity = match geometry.id {
            Some(id) => Identity::Id(id),
            None => Identity::Part(std::sync::Arc::as_ptr(&geometry.properties) as usize),
        };
        if !seen.insert((layer.id.clone(), identity)) {
            continue;
        }
        found.push((
            layer.index,
            QueriedFeature {
                layer: layer.id.clone(),
                source: layer.source.clone(),
                source_layer: geometry.source_layer.to_string(),
                id: geometry.id,
                properties: geometry
                    .properties
                    .iter()
                    .map(|(key, value)| (key.clone(), value.to_json()))
                    .collect(),
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
