//! Raster source coverings for projections and layers that read raster or DEM tiles.

use super::{FrameLodHistory, RasterCoverings};
use crate::{
    coords::{WorldTileCoords, TILE_SIZE},
    io::tile_sources::{raster_source_layer_indices, RasterSourceId, RASTER_LAYER_TYPES},
    render::{
        error::ProjectionStateError,
        projection::{covering_region_with_history, CoveringRequest},
        view_state::{ViewState, ViewStatePadding},
    },
    style::{source::Source, Style},
    tcs::world::World,
};

/// The tiles each raster source used by a visible layer covers, following that source's tile
/// size, rounding and zoom range through the same covering as the view.
pub fn raster_source_regions(
    style: &Style,
    view_state: &ViewState,
    world: &World,
    padding: ViewStatePadding,
) -> Result<RasterCoverings, ProjectionStateError> {
    if !style.cached_has_any_raster_type(&RASTER_LAYER_TYPES) {
        return Ok(Vec::new());
    }
    let zoom = view_state.zoom().value();
    let style_zoom = view_state.style_zoom().value();
    let mut regions = Vec::new();
    for (source_name, layer_indices) in raster_source_layer_indices(style).iter() {
        if !any_layer_visible(style, layer_indices, style_zoom) {
            continue;
        }
        let source = RasterSourceId::new(source_name.clone());
        let request = raster_request_for_source(style, &source, zoom, style_zoom);
        let tiles = covering_tiles_for_source(style, view_state, world, &source, request, padding)?;
        regions.push((source, tiles));
    }
    Ok(regions)
}

fn any_layer_visible(style: &Style, layer_indices: &[usize], style_zoom: f64) -> bool {
    layer_indices
        .iter()
        .filter_map(|&idx| style.layers.get(idx))
        .any(|layer| layer.is_visible_at(style_zoom))
}

fn raster_request_for_source(
    style: &Style,
    source: &RasterSourceId,
    zoom: f64,
    style_zoom: f64,
) -> CoveringRequest {
    let definition = source.name().and_then(|name| style.sources.get(name));
    let mut request = raster_covering_request(zoom, definition);
    if !request.zoom_range.serves(request.level) && style_zoom > zoom {
        request.level = raster_covering_request(style_zoom, definition).level;
    }
    request
}

fn covering_tiles_for_source(
    style: &Style,
    view_state: &ViewState,
    world: &World,
    source: &RasterSourceId,
    request: CoveringRequest,
    padding: ViewStatePadding,
) -> Result<Vec<WorldTileCoords>, ProjectionStateError> {
    let history = world
        .resources
        .get::<FrameLodHistory>()
        .and_then(|history| history.raster.get(source));
    Ok(
        covering_region_with_history(style, view_state, world, request, padding, history)?
            .map_or_else(Vec::new, |region| {
                region
                    .iter()
                    .filter(|coords| coords.build_quad_key().is_some())
                    .collect()
            }),
    )
}

fn raster_covering_request(zoom: f64, source: Option<&Source>) -> CoveringRequest {
    let (tile_size, minzoom, maxzoom) = match source {
        Some(Source::Raster(source)) => (
            source.tile_size.map_or(TILE_SIZE, f64::from),
            source.minzoom,
            source.maxzoom,
        ),
        Some(Source::RasterDem(source)) => (
            f64::from(source.tile_size()),
            source.minzoom,
            source.maxzoom,
        ),
        _ => (TILE_SIZE, None, None),
    };
    CoveringRequest::raster_source(zoom, tile_size, minzoom, maxzoom)
}
