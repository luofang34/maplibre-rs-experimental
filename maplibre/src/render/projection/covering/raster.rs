//! Raster source coverings for projections and layers that read raster or DEM tiles.

use super::{covering_region_with_history, ProjectionStateError};
use crate::{
    coords::TILE_SIZE,
    io::tile_sources::{source_layer_groups, RasterSourceId, TileKind, RASTER_LAYER_TYPES},
    render::{
        eye_covering::{FrameLodHistory, RasterCoverings},
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
    for group in source_layer_groups(style, TileKind::Raster) {
        if !group
            .layers
            .iter()
            .any(|layer| layer.is_visible_at(style_zoom))
        {
            continue;
        }
        let source = RasterSourceId::new(group.source_name);
        let definition = source.name().and_then(|name| style.sources.get(name));
        let mut request = raster_covering_request(zoom, definition);
        if !request.zoom_range.serves(request.level) && style_zoom > zoom {
            request.level = raster_covering_request(style_zoom, definition).level;
        }
        let history = world
            .resources
            .get::<FrameLodHistory>()
            .and_then(|history| history.raster.get(&source));
        let tiles =
            covering_region_with_history(style, view_state, world, request, padding, history)?
                .map_or_else(Vec::new, |region| {
                    region
                        .iter()
                        .filter(|coords| coords.build_quad_key().is_some())
                        .collect()
                });
        regions.push((source, tiles));
    }
    Ok(regions)
}

fn raster_covering_request(zoom: f64, source: Option<&Source>) -> super::CoveringRequest {
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
    super::CoveringRequest::raster_source(zoom, tile_size, minzoom, maxzoom)
}
