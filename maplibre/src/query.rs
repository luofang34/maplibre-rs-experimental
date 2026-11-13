//! Features under a screen point or box, as GL JS `queryRenderedFeatures`.
//!
//! Symbols come from the latest placement pass. Fill, line, circle, heatmap and extrusion features
//! come from the geometry index each worker builds for a tile and are evaluated against the live
//! style across flat Mercator, 3D terrain, and 3D globe / transition projections.

use std::{
    cmp::Reverse,
    collections::{BTreeMap, HashSet},
};

use serde::Serialize;

use crate::{
    coords::{WorldTileCoords, ZoomLevel, EXTENT},
    io::geometry_index::{ExactGeometry, IndexedGeometry},
    render::{resolved_view::ResolvedFrameView, view_state::ViewState},
    sdf::query::{
        query_rendered_symbols_in, QueryError, QueryGeometry, QueryOptions, RenderedSymbol,
    },
    style::{
        expression::Value,
        filter::{FeatureContext, Filter, GeometryType},
        layer::LayerPaint,
        source::GEOJSON_LAYER,
        Style,
    },
    tcs::world::World,
    terrain::coverage::TerrainCoverageIndex,
};

mod circle;
mod extrusion;
mod ground;
mod order;
mod overzoomed;
mod surface;
mod tiles;

use order::{in_drawing_order, Candidate};
use surface::{
    camera_query_bounds, resolve_terrain_index, surface_footprint, visible_in_screen_box,
};
use tiles::{candidates_in, tiles_in, QueryTile};

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
            id: symbol.id,
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
            (index, None, QueriedFeature::from(symbol))
        })
        .collect();
    found.extend(vector_features(
        world, style, view_state, geometry, options,
    )?);
    Ok(in_drawing_order(found))
}

/// A feature with its layer's index and, for a fill extrusion, its depth on screen.
type Found = (u32, Option<f64>, QueriedFeature);

/// Where a feature comes in GL JS's top-down order within its layer.
type TopDown = (u8, i32, i32, i32, Reverse<u32>);

/// What one query keeps while it walks tiles.
struct Search<'a> {
    candidates: Vec<Candidate<'a>>,
    query_filter: Option<Filter>,
    /// Features already reported, per layer and per tile: GL JS reports a feature once for each
    /// tile it is found in, merging only the copies of one tile in different worlds.
    seen: HashSet<(String, WorldTileCoords, Identity)>,
    /// Each feature with its layer's index and, for a fill extrusion, its depth on screen.
    found: Vec<(TopDown, Found)>,
    /// The query in window pixels, which extrusions and viewport circles are met against.
    screen: extrusion::ScreenQuery,
    /// The corners of the ground the query covers, in world pixels.
    ground: Vec<[f64; 2]>,
    /// Whether the query box surrounds the entire visible globe disc in space.
    whole_visible_hemisphere: bool,
    world: &'a World,
    resolved: &'a ResolvedFrameView,
    terrain_index: Option<&'a TerrainCoverageIndex>,
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
            matches!(
                layer.type_.as_str(),
                "fill" | "line" | "fill-extrusion" | "circle" | "heatmap"
            ) && layer.is_visible_at(zoom.value())
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
    let query_filter = options
        .filter
        .as_ref()
        .map(Filter::parse)
        .transpose()
        .map_err(QueryError::InvalidFilter)?;
    let Some(bounds) = geometry.bounds() else {
        return Err(QueryError::InvalidGeometry);
    };
    let Ok(resolved) = ResolvedFrameView::get_or_resolve(world, style, view_state) else {
        return Ok(Vec::new());
    };
    let terrain_index = resolve_terrain_index(world, style, &resolved);

    // An extrusion stands up from the ground, so it can meet the query far from where the
    // query meets the ground: every tile between the camera nadir and the query is searched.
    let extrudes = candidates
        .iter()
        .any(|candidate| candidate.layer.type_ == "fill-extrusion");
    let margin = candidates
        .iter()
        .map(Candidate::margin_pixels)
        .fold(0.0, f64::max);

    let (region, ground, whole_visible_hemisphere) = if extrudes {
        let searched = camera_query_bounds(view_state, &resolved, bounds);
        let Some(searched_fp) = surface_footprint(
            world,
            view_state,
            &resolved,
            terrain_index.as_deref(),
            searched,
        ) else {
            return Ok(Vec::new());
        };
        let query_fp = surface_footprint(
            world,
            view_state,
            &resolved,
            terrain_index.as_deref(),
            bounds,
        );
        let (corners, whole_hemi) = query_fp
            .map(|fp| (fp.corners, fp.whole_visible_hemisphere))
            .unwrap_or_default();
        (searched_fp.region, corners, whole_hemi)
    } else {
        let Some(fp) = surface_footprint(
            world,
            view_state,
            &resolved,
            terrain_index.as_deref(),
            bounds,
        ) else {
            return Ok(Vec::new());
        };
        (fp.region, fp.corners, fp.whole_visible_hemisphere)
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
        ground,
        whole_visible_hemisphere,
        world,
        resolved: &resolved,
        terrain_index: terrain_index.as_deref(),
    };
    search_tiles(world, (region, margin), &mut search);
    let mut found = search.found;
    found.sort_by_key(|(order, _)| *order);
    Ok(found.into_iter().map(|(_, feature)| feature).collect())
}

/// Collects from the finest level of tiles that holds any index for the region, looking
/// `margin` screen pixels beyond the query for features a layer draws within reach of it.
fn search_tiles(world: &World, (region, margin): ([f64; 4], f64), search: &mut Search) {
    let zoom = search.resolved.zoom;
    // One level above the view zoom covers sources whose tiles are finer than the view's.
    let top = (zoom.value().floor() as i32 + 1).clamp(0, MAX_QUERY_ZOOM);
    let Some(z) = (0..=top).rev().find(|&z| {
        tiles_in(region, margin, search.resolved, z as u8)
            .iter()
            .any(|tile| {
                world
                    .tiles
                    .geometry_index
                    .tile_indexes(&tile.coords)
                    .is_some()
            })
    }) else {
        return;
    };
    // A source that stops before the view's zoom is queried as GL JS queries it: through the
    // tiles of the view's zoom cut out of the coarser one.
    let view_z = (zoom.value().floor() as i32).clamp(0, MAX_QUERY_ZOOM);
    let tile_z = z.max(view_z) as u8;
    for mut tile in tiles_in(region, margin, search.resolved, tile_z) {
        let depth = tile_z - z as u8;
        let source_coords = WorldTileCoords {
            x: tile.coords.x >> depth,
            y: tile.coords.y >> depth,
            z: ZoomLevel::new(z as u8),
        };
        let Some(indexes) = world.tiles.geometry_index.tile_indexes(&source_coords) else {
            continue;
        };
        tile.footprint = if search.whole_visible_hemisphere {
            ground::Footprint::of(&[[0.0, 0.0], [EXTENT, 0.0], [EXTENT, EXTENT], [0.0, EXTENT]])
        } else {
            tile.footprint_of(&search.ground)
        };
        let tile = &tile;
        let scale = f64::from(1_u32 << depth);
        let offset = [
            f64::from(tile.coords.x - (source_coords.x << depth)) * EXTENT,
            f64::from(tile.coords.y - (source_coords.y << depth)) * EXTENT,
        ];
        let widen = margin * tile.units_per_pixel;
        let [x0, y0, x1, y1] = tile.local;
        // The window in the units of the tile the index belongs to.
        let child = [x0 - widen, y0 - widen, x1 + widen, y1 + widen];
        let window = [0, 1, 2, 3].map(|i| (child[i] + offset[i % 2]) / scale);
        for (source, index) in indexes {
            for geometry in candidates_in(index, window) {
                if depth == 0 {
                    collect(geometry, source, tile, search);
                    continue;
                }
                for part in overzoomed::slice(geometry, scale, offset) {
                    collect(&part, source, tile, search);
                }
            }
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
        ExactGeometry::Point(_) => (GeometryType::Point, "Point"),
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
        whole_visible_hemisphere,
        world,
        resolved,
        terrain_index,
        ..
    } = search;
    let zoom_val = resolved.zoom.value();
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
        let visible_check = |cand: &Candidate| {
            !*whole_visible_hemisphere
                || visible_in_screen_box(
                    geometry,
                    tile.origin,
                    tile.world_per_unit,
                    cand.translate,
                    (world, resolved, *terrain_index, *screen),
                )
        };
        let depth = match (&geometry.exact, candidate.layer.type_.as_str()) {
            (ExactGeometry::Polygon(polygon), "fill-extrusion") => {
                match extrusion::extruded_depth(
                    polygon,
                    tile,
                    candidate,
                    (
                        &geometry.properties,
                        *screen,
                        world,
                        resolved,
                        *terrain_index,
                    ),
                ) {
                    Some(depth) => Some(depth),
                    None => continue,
                }
            }
            (exact, "heatmap") => match &candidate.layer.paint {
                Some(LayerPaint::Heatmap(paint))
                    if circle::heatmap_hit(
                        exact,
                        tile,
                        (paint, &geometry.properties),
                        zoom_val,
                    ) && visible_check(candidate) =>
                {
                    None
                }
                _ => continue,
            },
            (exact, "circle") => match &candidate.layer.paint {
                Some(LayerPaint::Circle(paint))
                    if circle::geometry_hit(
                        exact,
                        (tile, candidate),
                        (paint, &geometry.properties),
                        (*screen, world, resolved, *terrain_index),
                    ) =>
                {
                    None
                }
                _ => continue,
            },
            _ if flat_hit(geometry, tile, candidate, zoom_val) && visible_check(candidate) => None,
            _ => continue,
        };
        let identity = match geometry.id {
            Some(id) => Identity::Id(id),
            None => Identity::Part(std::sync::Arc::as_ptr(&geometry.properties) as usize),
        };
        if !seen.insert((layer.id.clone(), tile.coords, identity)) {
            continue;
        }
        let order = (
            u8::from(tile.coords.z),
            tile.coords.y,
            tile.wrap,
            tile.coords.x,
            Reverse(geometry.feature_index),
        );
        found.push((
            order,
            (
                layer.index,
                depth,
                QueriedFeature {
                    layer: layer.id.clone(),
                    source: layer.source.clone(),
                    source_layer: geometry.source_layer.to_string(),
                    id: geometry.id,
                    properties: source_properties(&geometry.properties),
                    state: crate::style::feature_state::extract_feature_state(&geometry.properties),
                    geometry_type,
                    text: None,
                },
            ),
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
