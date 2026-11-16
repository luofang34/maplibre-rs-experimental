//! Geometry uploads follow the actual sources selected for the view or terrain textures,
//! prioritizing uncovered visible tiles first and retaining coarse overview / 1-ring stand-ins.
use rustc_hash::FxHashSet;

use crate::{
    coords::WorldTileCoords,
    io::tile_sources::{clamp_to_max_zoom, source_max_zoom, TileKind},
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::view_region_for_projection,
        tile_view_pattern::{coverage::loaded_cover, HasTile, ViewTileSources, DEFAULT_TILE_SIZE},
        view_state::{ViewState, ViewStatePadding},
    },
    style::Style,
    tcs::{system::SystemError, world::World},
    terrain::{
        drape_targets::select_targets, queue_system::uniforms::present_ancestors,
        request_system::DrapeRequests, resources::TerrainResources,
    },
};

const CHILDREN_SEARCH_DEPTH: usize = 4;

/// Coarse overview and settled 1-ring prefetch tiles requested by `vector_request` so
/// `upload_system` uploads and retains them in `VectorBufferPool` against pan holes.
#[derive(Clone, Default)]
pub(crate) struct VectorFallbackTiles(pub Vec<WorldTileCoords>);

pub(super) type FrameSourcesAndPainted = (
    Vec<WorldTileCoords>,
    Vec<WorldTileCoords>,
    FxHashSet<WorldTileCoords>,
);

pub(super) fn frame_sources_and_painted(
    world: &World,
    style: &Style,
    view_state: &ViewState,
) -> Result<FrameSourcesAndPainted, SystemError> {
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
    let (source_tiles, uncovered) = if style.terrain.is_some() {
        terrain_sources(world, style)
    } else {
        let mut uncovered_list = Vec::new();
        let mut covered_list = Vec::new();
        let mut uncovered_set = FxHashSet::default();
        let mut seen = FxHashSet::default();
        let mut uncovered_targets = Vec::new();
        let mut covered_targets = Vec::new();

        if let Some(region) = &view_region {
            for copy in region.copies() {
                let Some((coords, _)) = copy.wrapped() else {
                    continue;
                };
                let has_gpu_stand_in = loaded_cover(&gpu_vector, coords, world).is_some()
                    || gpu_vector
                        .get_available_children(coords, world, CHILDREN_SEARCH_DEPTH)
                        .is_some();
                if has_gpu_stand_in {
                    covered_targets.push(coords);
                } else {
                    uncovered_targets.push(coords);
                }
                for ready in loaded_cover(&cpu_vector, coords, world)
                    .or_else(|| {
                        cpu_vector.get_available_children(coords, world, CHILDREN_SEARCH_DEPTH)
                    })
                    .into_iter()
                    .flatten()
                {
                    if !has_gpu_stand_in {
                        uncovered_set.insert(ready);
                    }
                    if seen.insert(ready) {
                        if has_gpu_stand_in {
                            covered_list.push(ready);
                        } else {
                            uncovered_list.push(ready);
                        }
                    }
                }
            }
        }
        for coords in uncovered_targets {
            uncovered_set.insert(coords);
            if seen.insert(coords) {
                uncovered_list.push(coords);
            }
        }
        for coords in covered_targets {
            if seen.insert(coords) {
                covered_list.push(coords);
            }
        }
        if let Some(fallbacks) = world.resources.get::<VectorFallbackTiles>() {
            for &coords in &fallbacks.0 {
                if seen.insert(coords) {
                    covered_list.push(coords);
                }
            }
        }
        uncovered_list.extend(covered_list);
        (uncovered_list, uncovered_set)
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
    Ok((source_tiles, painted_tiles, uncovered))
}

pub(super) fn terrain_sources(
    world: &World,
    style: &Style,
) -> (Vec<WorldTileCoords>, FxHashSet<WorldTileCoords>) {
    let max_zoom = source_max_zoom(style, TileKind::Vector);
    let terrain = match world.resources.get::<Eventually<TerrainResources>>() {
        Some(Initialized(t)) => Some(t),
        _ => None,
    };
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
    let mut uncovered_list = Vec::new();
    let mut covered_list = Vec::new();
    let mut uncovered_set = FxHashSet::default();
    for (target, shapes) in selected {
        let is_uncovered = terrain.is_some_and(|t| {
            t.drape_texture(target).is_none() && present_ancestors(target, t).is_empty()
        });
        for source in shapes
            .into_iter()
            .filter(|shape| shape.raster_source.is_none())
            .map(|shape| shape.coords)
            .chain(std::iter::once(clamp_to_max_zoom(target, max_zoom)))
        {
            if is_uncovered {
                uncovered_set.insert(source);
            }
            if seen.insert(source) {
                if is_uncovered {
                    uncovered_list.push(source);
                } else {
                    covered_list.push(source);
                }
            }
        }
    }
    uncovered_list.extend(covered_list);
    (uncovered_list, uncovered_set)
}
