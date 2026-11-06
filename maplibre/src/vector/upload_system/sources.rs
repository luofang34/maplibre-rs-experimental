//! Geometry uploads follow the actual sources selected for the view or terrain textures.
use rustc_hash::FxHashSet;

use crate::{
    coords::WorldTileCoords,
    io::tile_sources::{clamp_to_max_zoom, source_max_zoom, TileKind},
    render::{
        projection::view_region_for_projection,
        tile_view_pattern::{coverage::loaded_cover, HasTile, ViewTileSources, DEFAULT_TILE_SIZE},
        view_state::{ViewState, ViewStatePadding},
    },
    style::Style,
    tcs::{system::SystemError, world::World},
    terrain::{drape_targets::select_targets, request_system::DrapeRequests},
};

const CHILDREN_SEARCH_DEPTH: usize = 4;

pub(super) fn frame_sources_and_painted(
    world: &World,
    style: &Style,
    view_state: &ViewState,
) -> Result<(Vec<WorldTileCoords>, Vec<WorldTileCoords>), SystemError> {
    let view_region = view_region_for_projection(
        style,
        view_state,
        world,
        view_state.zoom().zoom_level(DEFAULT_TILE_SIZE),
        ViewStatePadding::Tight,
    )
    .map_err(|error| {
        tracing::error!(%error, "unable to select tiles for vector upload");
        SystemError::Setup
    })?;
    let Some(view_tile_sources) = world.resources.get::<ViewTileSources>() else {
        return Err(SystemError::Dependencies);
    };
    let cpu_vector = view_tile_sources.of_kind(TileKind::Vector);
    let gpu_vector = (
        view_tile_sources.of_kind(TileKind::Vector),
        crate::vector::UploadedVectorGeometry::for_world(world),
    );
    let source_tiles = if style.terrain.is_some() {
        terrain_sources(world, style)
    } else {
        let mut sources = Vec::new();
        let mut seen = FxHashSet::default();
        let mut targets = Vec::new();
        if let Some(region) = &view_region {
            for copy in region.copies() {
                let Some((coords, _)) = copy.wrapped() else {
                    continue;
                };
                targets.push(coords);
                for ready in loaded_cover(&cpu_vector, coords, world)
                    .or_else(|| {
                        cpu_vector.get_available_children(coords, world, CHILDREN_SEARCH_DEPTH)
                    })
                    .into_iter()
                    .flatten()
                {
                    if seen.insert(ready) {
                        sources.push(ready);
                    }
                }
            }
        }
        for coords in targets {
            if seen.insert(coords) {
                sources.push(coords);
            }
        }
        sources
    };
    let mut painted_tiles = source_tiles.clone();
    let mut seen_painted: FxHashSet<_> = painted_tiles.iter().copied().collect();
    if let Some(region) = &view_region {
        for copy in region.copies() {
            let Some((coords, _)) = copy.wrapped() else {
                continue;
            };
            for stand_in in loaded_cover(&gpu_vector, coords, world)
                .or_else(|| gpu_vector.get_available_children(coords, world, CHILDREN_SEARCH_DEPTH))
                .into_iter()
                .flatten()
            {
                if seen_painted.insert(stand_in) {
                    painted_tiles.push(stand_in);
                }
            }
        }
    }
    Ok((source_tiles, painted_tiles))
}

pub(super) fn terrain_sources(world: &World, style: &Style) -> Vec<WorldTileCoords> {
    let max_zoom = source_max_zoom(style, TileKind::Vector);
    let targets = std::iter::once(WorldTileCoords::default())
        .chain(
            world
                .resources
                .get::<DrapeRequests>()
                .into_iter()
                .flat_map(|requests| requests.0.iter().copied()),
        )
        .chain(
            world
                .resources
                .get::<crate::render::eye_covering::SlotResidencyFloor>()
                .into_iter()
                .flat_map(|floor| floor.retained_drapes.iter().copied()),
        );
    let selected = select_targets(targets, world, &[]);
    let mut seen = FxHashSet::default();
    let mut sources = Vec::new();
    for (target, shapes) in selected {
        // A CPU-ready ancestor or complete child cover must reach the GPU even when
        // the requested tile exceeds maxzoom or has not arrived yet.
        for source in shapes
            .into_iter()
            .filter(|shape| shape.raster_source.is_none())
            .map(|shape| shape.coords)
            .chain(std::iter::once(clamp_to_max_zoom(target, max_zoom)))
        {
            if seen.insert(source) {
                sources.push(source);
            }
        }
    }
    sources
}
