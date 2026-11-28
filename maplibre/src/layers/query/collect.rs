use std::{cmp::Reverse, collections::HashSet};

use super::{
    circle, extrusion,
    globe::visible_in_screen_box,
    ground,
    order::Candidate,
    overzoomed, source_promote_property, source_properties,
    surface::{camera_query_bounds, resolve_terrain_index, surface_footprint, ScreenQuery},
    tiles::{candidates_in, select_query_entries, QueryTile},
    QueriedFeature, QueriedFeatureId,
};
use crate::{
    coords::{WorldTileCoords, EXTENT},
    io::geometry_index::{ExactGeometry, IndexedGeometry},
    render::{resolved_view::ResolvedFrameView, view_state::ViewState},
    sdf::query::{QueryError, QueryGeometry, QueryOptions},
    style::{
        filter::{FeatureContext, Filter, GeometryType},
        layer::LayerPaint,
        source::GEOJSON_LAYER,
        Style,
    },
    tcs::world::World,
    terrain::coverage::TerrainCoverageIndex,
};

/// A feature with its layer's index and, for a fill extrusion, its depth on screen.
pub(super) type Found = (u32, Option<f64>, QueriedFeature);

/// Where a feature comes in GL JS's top-down order within its layer.
type TopDown = (u8, i32, i32, i32, Reverse<u32>);

/// What one query keeps while it walks tiles.
struct Search<'a> {
    candidates: Vec<Candidate<'a>>,
    query_filter: Option<Filter>,
    /// Features already reported, per layer and per tile.
    seen: HashSet<(String, WorldTileCoords, Identity)>,
    found: Vec<(TopDown, Found)>,
    screen: ScreenQuery,
    ground: Vec<[f64; 2]>,
    whole_visible_hemisphere: bool,
    view_state: &'a ViewState,
    world: &'a World,
    style: &'a Style,
    resolved: &'a ResolvedFrameView,
    terrain_index: Option<&'a TerrainCoverageIndex>,
}

/// What makes two parts one feature: their shared id, or the property map they share.
#[derive(PartialEq, Eq, Hash)]
enum Identity {
    Id(QueriedFeatureId),
    Part(usize),
}

fn select_candidates<'a>(
    style: &'a Style,
    view_state: &ViewState,
    options: &QueryOptions,
) -> Vec<Candidate<'a>> {
    let zoom = view_state.zoom().value();
    let bearing = view_state.camera().get_bearing().0;
    style
        .layers
        .iter()
        .filter(|layer| {
            matches!(
                layer.type_.as_str(),
                "fill" | "line" | "fill-extrusion" | "circle" | "heatmap"
            ) && layer.is_visible_at(zoom)
                && options
                    .layers
                    .as_ref()
                    .is_none_or(|layers| layers.contains(&layer.id))
        })
        .filter_map(|layer| Candidate::new(layer, zoom, bearing))
        .collect()
}

fn resolve_search_footprint(
    world: &World,
    view_state: &ViewState,
    resolved: &ResolvedFrameView,
    terrain_index: Option<&TerrainCoverageIndex>,
    bounds: [f64; 4],
    extrudes: bool,
) -> Option<([f64; 4], Vec<[f64; 2]>, bool)> {
    // An extrusion stands up from the ground, so it can meet the query far from where the
    // query meets the ground: every tile between the camera nadir and the query is searched.
    if extrudes {
        let searched = camera_query_bounds(view_state, resolved, bounds);
        let searched_fp = surface_footprint(world, view_state, resolved, terrain_index, searched)?;
        let (corners, whole_hemi) =
            surface_footprint(world, view_state, resolved, terrain_index, bounds)
                .map(|fp| (fp.corners, fp.whole_visible_hemisphere))
                .unwrap_or_default();
        Some((searched_fp.region, corners, whole_hemi))
    } else {
        let fp = surface_footprint(world, view_state, resolved, terrain_index, bounds)?;
        Some((fp.region, fp.corners, fp.whole_visible_hemisphere))
    }
}

pub(super) fn vector_features(
    world: &World,
    style: &Style,
    view_state: &ViewState,
    geometry: QueryGeometry,
    options: &QueryOptions,
) -> Result<Vec<Found>, QueryError> {
    let candidates = select_candidates(style, view_state, options);
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
    let extrudes = candidates
        .iter()
        .any(|candidate| candidate.layer.type_ == "fill-extrusion");
    let margin = candidates
        .iter()
        .map(|candidate| candidate.margin_pixels)
        .fold(0.0, f64::max);
    let Some((region, ground, whole_visible_hemisphere)) = resolve_search_footprint(
        world,
        view_state,
        &resolved,
        terrain_index.as_deref(),
        bounds,
        extrudes,
    ) else {
        return Ok(Vec::new());
    };
    let screen = match geometry {
        QueryGeometry::Point(point) => ScreenQuery::Point(point),
        QueryGeometry::Box { .. } => ScreenQuery::Box(bounds),
    };
    let mut search = Search {
        candidates,
        query_filter,
        seen: HashSet::new(),
        found: Vec::new(),
        screen,
        ground,
        whole_visible_hemisphere,
        view_state,
        world,
        style,
        resolved: &resolved,
        terrain_index: terrain_index.as_deref(),
    };
    search_tiles(world, (region, margin), &mut search);
    let mut found = search.found;
    found.sort_by_key(|(order, _)| *order);
    Ok(found.into_iter().map(|(_, feature)| feature).collect())
}

fn search_tiles(world: &World, (region, margin): ([f64; 4], f64), search: &mut Search) {
    for mut entry in select_query_entries(world, region, margin, search.resolved) {
        let Some(indexes) = world.tiles.as_ref().tile_indexes(&entry.source_coords) else {
            continue;
        };
        let depth = entry.depth;
        entry.tile.footprint = if search.whole_visible_hemisphere {
            ground::Footprint::of(&[[0.0, 0.0], [EXTENT, 0.0], [EXTENT, EXTENT], [0.0, EXTENT]])
        } else {
            entry.tile.footprint_of(&search.ground)
        };
        let tile = &entry.tile;
        let scale = f64::from(1_u32 << depth);
        let [ox, oy] = [
            f64::from(tile.coords.x - (entry.source_coords.x << depth)) * EXTENT,
            f64::from(tile.coords.y - (entry.source_coords.y << depth)) * EXTENT,
        ];
        let widen = margin * tile.units_per_pixel;
        let [x0, y0, x1, y1] = tile.local;
        let window = [
            (x0 - widen + ox) / scale,
            (y0 - widen + oy) / scale,
            (x1 + widen + ox) / scale,
            (y1 + widen + oy) / scale,
        ];
        for (source, index) in indexes {
            for geometry in candidates_in(index, window) {
                if depth == 0 {
                    collect(geometry, source, tile, search);
                    continue;
                }
                for part in overzoomed::slice(geometry, scale, [ox, oy]) {
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
        .map(|pixels| pixels / tile.world_per_unit);
    ground::touches(
        geometry,
        footprint,
        (candidate.layer.type_.as_str(), translate),
        line,
    )
}

fn candidate_matches(
    candidate: &Candidate,
    geometry: &IndexedGeometry<f64>,
    source: Option<&str>,
    context: &FeatureContext,
    query_filter: Option<&Filter>,
) -> bool {
    let layer = candidate.layer;
    layer.source.as_deref() == source
        && layer.source_layer.as_deref().unwrap_or(GEOJSON_LAYER) == &*geometry.source_layer
        && candidate
            .filter
            .as_ref()
            .is_none_or(|filter| filter.evaluate(context))
        && query_filter
            .as_ref()
            .is_none_or(|filter| filter.evaluate(context))
}

fn layer_reach_px(
    layer: &crate::style::layer::StyleLayer,
    geometry: &IndexedGeometry<f64>,
    zoom: f64,
) -> f64 {
    match &layer.paint {
        Some(LayerPaint::Line(paint)) => {
            let [hw, off] = ground::line_reach(paint, &geometry.properties, zoom);
            hw + off.abs()
        }
        Some(LayerPaint::Heatmap(paint)) => {
            super::order::heatmap_radius(paint, &geometry.properties, zoom)
        }
        _ => 0.0,
    }
}

fn candidate_hit_depth(
    candidate: &Candidate,
    geometry: &IndexedGeometry<f64>,
    tile: &QueryTile,
    search: &Search,
    zoom_val: f64,
) -> Option<Option<f64>> {
    let layer = candidate.layer;
    let reach_px = layer_reach_px(layer, geometry, zoom_val);
    let is_visible = || {
        !search.whole_visible_hemisphere
            || visible_in_screen_box(
                geometry,
                layer.type_.as_str(),
                (
                    tile.origin,
                    tile.world_per_unit,
                    candidate.translate,
                    reach_px,
                ),
                (
                    search.view_state,
                    search.world,
                    search.resolved,
                    search.terrain_index,
                    search.screen,
                ),
            )
    };
    match (&geometry.exact, layer.type_.as_str()) {
        (ExactGeometry::Polygon(polygon), "fill-extrusion") => extrusion::extruded_depth(
            polygon,
            tile,
            candidate,
            (
                &geometry.properties,
                search.screen,
                search.world,
                search.resolved,
                search.terrain_index,
            ),
        )
        .map(Some),
        (exact, "heatmap") => match &layer.paint {
            Some(LayerPaint::Heatmap(paint))
                if circle::heatmap_hit(exact, tile, (paint, &geometry.properties), zoom_val)
                    && is_visible() =>
            {
                Some(None)
            }
            _ => None,
        },
        (exact, "circle") => match &layer.paint {
            Some(LayerPaint::Circle(paint))
                if circle::geometry_hit(
                    exact,
                    (tile, candidate),
                    (paint, &geometry.properties),
                    (
                        search.screen,
                        search.world,
                        search.resolved,
                        search.terrain_index,
                        search.whole_visible_hemisphere,
                    ),
                ) =>
            {
                Some(None)
            }
            _ => None,
        },
        _ if flat_hit(geometry, tile, candidate, zoom_val) && is_visible() => Some(None),
        _ => None,
    }
}

fn collect(
    geometry: &IndexedGeometry<f64>,
    source: Option<&str>,
    tile: &QueryTile,
    search: &mut Search,
) {
    let kind = match &geometry.exact {
        ExactGeometry::Polygon(_) => GeometryType::Polygon,
        ExactGeometry::LineString(_) => GeometryType::LineString,
        ExactGeometry::Point(_) => GeometryType::Point,
    };
    let promote_prop = source_promote_property(search.style, source, &geometry.source_layer);
    let queried_id = if let Some(prop) = promote_prop {
        geometry
            .properties
            .get(prop)
            .and_then(QueriedFeatureId::from_expr_value)
    } else {
        geometry.id.map(QueriedFeatureId::Number)
    };
    let context = FeatureContext {
        properties: &geometry.properties,
        geometry_type: kind,
        id: queried_id.as_ref().map(QueriedFeatureId::to_expr_value),
        zoom: f64::from(tile.zoom_level),
    };
    let zoom_val = search.resolved.zoom_value();
    let mut hits = Vec::new();
    for candidate in &search.candidates {
        if !candidate_matches(
            candidate,
            geometry,
            source,
            &context,
            search.query_filter.as_ref(),
        ) {
            continue;
        }
        let Some(depth) = candidate_hit_depth(candidate, geometry, tile, search, zoom_val) else {
            continue;
        };
        hits.push((
            candidate.layer.index,
            &candidate.layer.id,
            &candidate.layer.source,
            depth,
        ));
    }
    for (layer_index, layer_id, layer_source, depth) in hits {
        record_query_hit(
            geometry,
            tile,
            &queried_id,
            (layer_index, layer_id, layer_source, depth),
            search,
        );
    }
}

fn record_query_hit(
    geometry: &IndexedGeometry<f64>,
    tile: &QueryTile,
    queried_id: &Option<QueriedFeatureId>,
    (layer_index, layer_id, layer_source, depth): (u32, &str, &Option<String>, Option<f64>),
    search: &mut Search,
) {
    let geometry_type = match &geometry.exact {
        ExactGeometry::Polygon(_) => "Polygon",
        ExactGeometry::LineString(_) => "LineString",
        ExactGeometry::Point(_) => "Point",
    };
    let identity = match queried_id {
        Some(id) => Identity::Id(id.clone()),
        None => Identity::Part(std::sync::Arc::as_ptr(&geometry.properties) as usize),
    };
    if !search
        .seen
        .insert((layer_id.to_owned(), tile.coords, identity))
    {
        return;
    }
    let order = (
        u8::from(tile.coords.z),
        tile.coords.y,
        tile.wrap,
        tile.coords.x,
        Reverse(geometry.feature_index),
    );
    search.found.push((
        order,
        (
            layer_index,
            depth,
            QueriedFeature {
                layer: layer_id.to_owned(),
                source: layer_source.clone(),
                source_layer: geometry.source_layer.to_string(),
                id: queried_id.clone(),
                properties: source_properties(&geometry.properties),
                state: crate::style::feature_state::extract_feature_state(&geometry.properties),
                geometry_type,
                text: None,
            },
        ),
    ));
}
