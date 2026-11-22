//! Geometry uploads follow the actual sources selected for the view or terrain textures,
//! prioritizing uncovered visible tiles first and retaining coarse overview / 1-ring stand-ins.
use std::sync::Arc;

use rustc_hash::FxHashSet;

use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    io::tile_sources::{clamp_to_max_zoom, source_max_zoom, TileKind},
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::view_region_for_projection,
        tile_view_pattern::{
            coverage::loaded_cover, HasTile, ViewTileSources, CHILDREN_SEARCH_DEPTH,
            DEFAULT_TILE_SIZE,
        },
        view_state::{ViewState, ViewStatePadding},
    },
    style::Style,
    tcs::{system::SystemError, world::World},
    terrain::{
        drape_targets::select_targets, queue_system::uniforms::has_present_ancestor,
        request_system::DrapeRequests, resources::TerrainResources,
    },
};

/// Coarse overview and settled 1-ring prefetch tiles requested by `vector_request` so
/// `upload_system` uploads and retains them in `VectorBufferPool` against pan holes.
#[derive(Clone, Default)]
pub(crate) struct VectorFallbackTiles(pub Vec<WorldTileCoords>);

pub(super) type FrameSourcesAndPainted = (
    Vec<WorldTileCoords>,
    Vec<WorldTileCoords>,
    FxHashSet<WorldTileCoords>,
);

type FreshTightTiles = (ZoomLevel, Arc<[WorldTileCoords]>);

#[cfg(all(test, feature = "headless", feature = "thread-safe-futures"))]
pub(super) fn frame_sources_and_painted(
    world: &World,
    style: &Style,
    view_state: &ViewState,
) -> Result<FrameSourcesAndPainted, SystemError> {
    Ok(compute_frame_sources_and_painted(world, style, view_state)?.0)
}

pub(super) fn frame_sources_and_painted_mut(
    world: &mut World,
    style: &Style,
    view_state: &ViewState,
) -> Result<FrameSourcesAndPainted, SystemError> {
    let (result, fresh_tight) = compute_frame_sources_and_painted(world, style, view_state)?;
    if let Some((level, tiles)) = fresh_tight {
        let host_frame = crate::render::frame_signals::host_frame_in_progress(world);
        let cache = world
            .resources
            .get_or_init_mut::<crate::render::eye_covering::FrameLooseCovering>();
        if cache.host_frame != host_frame {
            *cache = crate::render::eye_covering::FrameLooseCovering::for_host_frame(host_frame);
        }
        cache.tight_view_tiles = Some((Some(tiles), ViewStatePadding::Tight, level));
    }
    Ok(result)
}

fn compute_frame_sources_and_painted(
    world: &World,
    style: &Style,
    view_state: &ViewState,
) -> Result<(FrameSourcesAndPainted, Option<FreshTightTiles>), SystemError> {
    let Some(view_tile_sources) = world.resources.get::<ViewTileSources>() else {
        return Err(SystemError::Dependencies);
    };
    let cpu_vector = view_tile_sources.of_kind(TileKind::Vector);
    let gpu_vector = (
        view_tile_sources.of_kind(TileKind::Vector),
        crate::vector::UploadedVectorGeometry::for_world(world),
    );
    let (view_tiles, fresh_tight) = resolve_view_tiles_for_paint(world, style, view_state)?;
    let (source_tiles, mut painted_tiles, uncovered, all_view_painted) = if style.terrain.is_some()
    {
        let (src, uncov) = terrain_sources(world, style);
        let painted = src.clone();
        (src, painted, uncov, false)
    } else {
        partition_non_terrain_sources(world, view_tiles.as_deref(), &cpu_vector, &gpu_vector)
    };
    if !all_view_painted {
        if let Some(tiles) = &view_tiles {
            append_missing_gpu_stand_ins(world, tiles, &gpu_vector, &mut painted_tiles);
        }
    }
    Ok(((source_tiles, painted_tiles, uncovered), fresh_tight))
}

fn resolve_cached_wrapped_tiles(
    world: &World,
    style: &Style,
    view_state: &ViewState,
    visible_level: ZoomLevel,
    multi_active: bool,
) -> Option<Option<Vec<WorldTileCoords>>> {
    let host_frame = crate::render::frame_signals::host_frame_in_progress(world);
    if !multi_active {
        if let Some(tiles) = world
            .resources
            .get::<crate::render::eye_covering::FrameLooseCovering>()
            .filter(|cache| cache.host_frame == host_frame)
            .and_then(|cache| {
                cache
                    .tight_view_tiles
                    .as_ref()
                    .or(cache.view_tiles.as_ref())
            })
            .filter(|(_, padding, cached_level)| {
                *padding == ViewStatePadding::Tight && *cached_level == visible_level
            })
            .map(|(tiles, _, _)| {
                tiles.as_ref().map(|tiles| {
                    tiles
                        .iter()
                        .filter_map(|coords| coords.wrapped().map(|(wrapped, _)| wrapped))
                        .collect()
                })
            })
        {
            return Some(tiles);
        }
    }
    crate::render::eye_covering::matching_tight_tiles(world, style, view_state, visible_level).map(
        |shared_tiles| {
            Some(
                shared_tiles
                    .iter()
                    .filter_map(|coords| coords.wrapped().map(|(w, _)| w))
                    .collect(),
            )
        },
    )
}

fn resolve_view_tiles_for_paint(
    world: &World,
    style: &Style,
    view_state: &ViewState,
) -> Result<(Option<Vec<WorldTileCoords>>, Option<FreshTightTiles>), SystemError> {
    let needs_view_tiles_for_paint = style.terrain.is_none()
        || style.layers.iter().any(|l| {
            !l.is_hidden()
                && !matches!(
                    l.type_.as_str(),
                    "background" | "symbol" | "sky" | "raster" | "hillshade" | "color-relief"
                )
                && (!crate::terrain::is_drapeable(&l.type_)
                    || crate::vector::structures::kind(l).is_some())
        });
    if !needs_view_tiles_for_paint {
        return Ok((None, None));
    }
    let visible_level = view_state.zoom().zoom_level(DEFAULT_TILE_SIZE);
    let multi_active = world
        .resources
        .get::<crate::render::eye_covering::MultiViewExtractCovering>()
        .is_some_and(|m| m.active);
    if let Some(tiles) =
        resolve_cached_wrapped_tiles(world, style, view_state, visible_level, multi_active)
    {
        return Ok((tiles, None));
    }
    let region = view_region_for_projection(
        style,
        view_state,
        world,
        visible_level,
        ViewStatePadding::Tight,
    )
    .map_err(|error| {
        tracing::error!(%error, "unable to select tiles for vector upload");
        SystemError::Setup
    })?;
    let wrapped = region.as_ref().map(|r| {
        r.copies()
            .filter_map(|copy| copy.wrapped().map(|(coords, _)| coords))
            .collect()
    });
    let fresh = if !multi_active {
        region
            .as_ref()
            .map(|r| (visible_level, Arc::from(r.iter().collect::<Vec<_>>())))
    } else {
        None
    };
    Ok((wrapped, fresh))
}

#[derive(Default)]
struct NonTerrainPartition {
    uncovered_list: Vec<WorldTileCoords>,
    covered_list: Vec<WorldTileCoords>,
    uncovered_set: FxHashSet<WorldTileCoords>,
    seen: FxHashSet<WorldTileCoords>,
    uncovered_targets: Vec<WorldTileCoords>,
    covered_targets: Vec<WorldTileCoords>,
}

fn partition_non_terrain_sources(
    world: &World,
    view_tiles: Option<&[WorldTileCoords]>,
    cpu_vector: &impl HasTile,
    gpu_vector: &impl HasTile,
) -> (
    Vec<WorldTileCoords>,
    Vec<WorldTileCoords>,
    FxHashSet<WorldTileCoords>,
    bool,
) {
    let mut part = NonTerrainPartition::default();
    if let Some(tiles) = view_tiles {
        collect_view_tile_sources(world, tiles, cpu_vector, gpu_vector, &mut part);
    }
    let all_view_painted = part.uncovered_targets.is_empty() && part.covered_targets.is_empty();
    for coords in std::mem::take(&mut part.uncovered_targets) {
        part.uncovered_set.insert(coords);
        if part.seen.insert(coords) {
            part.uncovered_list.push(coords);
        }
        part.uncovered_targets.push(coords);
    }
    for coords in part.covered_targets.drain(..) {
        if part.seen.insert(coords) {
            part.covered_list.push(coords);
        }
    }
    let coarse_uncovered = collect_fallback_sources(world, cpu_vector, gpu_vector, &mut part);
    let mut painted = Vec::with_capacity(
        coarse_uncovered.len() + part.uncovered_list.len() + part.covered_list.len(),
    );
    painted.extend_from_slice(&coarse_uncovered);
    painted.extend_from_slice(&part.uncovered_list);
    painted.extend_from_slice(&part.covered_list);
    let mut upload_order = coarse_uncovered;
    upload_order.extend(part.uncovered_list);
    upload_order.extend(part.covered_list);
    (upload_order, painted, part.uncovered_set, all_view_painted)
}

fn collect_view_tile_sources(
    world: &World,
    tiles: &[WorldTileCoords],
    cpu_vector: &impl HasTile,
    gpu_vector: &impl HasTile,
    part: &mut NonTerrainPartition,
) {
    for &coords in tiles {
        if gpu_vector.has_tile(coords, world) && cpu_vector.has_tile(coords, world) {
            if part.seen.insert(coords) {
                part.covered_list.push(coords);
            }
            continue;
        }
        let has_gpu_stand_in = gpu_vector.has_tile(coords, world)
            || loaded_cover(gpu_vector, coords, world).is_some()
            || gpu_vector
                .get_available_children(coords, world, CHILDREN_SEARCH_DEPTH)
                .is_some_and(|children| !children.is_empty());
        if has_gpu_stand_in {
            part.covered_targets.push(coords);
        } else {
            part.uncovered_targets.push(coords);
        }
        for ready in loaded_cover(cpu_vector, coords, world)
            .or_else(|| {
                cpu_vector
                    .get_available_children(coords, world, CHILDREN_SEARCH_DEPTH)
                    .filter(|children| !children.is_empty())
            })
            .into_iter()
            .flatten()
        {
            if !has_gpu_stand_in {
                part.uncovered_set.insert(ready);
            }
            if part.seen.insert(ready) {
                if has_gpu_stand_in {
                    part.covered_list.push(ready);
                } else {
                    part.uncovered_list.push(ready);
                }
            }
        }
    }
}

fn collect_fallback_sources(
    world: &World,
    cpu_vector: &impl HasTile,
    gpu_vector: &impl HasTile,
    part: &mut NonTerrainPartition,
) -> Vec<WorldTileCoords> {
    let mut coarse_uncovered = Vec::new();
    let Some(fallbacks) = world.resources.get::<VectorFallbackTiles>() else {
        return coarse_uncovered;
    };
    for &coords in &fallbacks.0 {
        let covers_uncovered = !part.uncovered_targets.is_empty()
            && part.uncovered_targets.iter().any(|&target| {
                coords.z < target.z && {
                    let shift = u32::from(u8::from(target.z) - u8::from(coords.z));
                    target.x >> shift == coords.x && target.y >> shift == coords.y
                }
            });
        if covers_uncovered
            && cpu_vector.has_tile(coords, world)
            && !gpu_vector.has_tile(coords, world)
        {
            if part.seen.insert(coords) {
                part.uncovered_set.insert(coords);
                coarse_uncovered.push(coords);
            }
        } else if part.seen.insert(coords) {
            part.covered_list.push(coords);
        }
    }
    coarse_uncovered
}

fn append_missing_gpu_stand_ins(
    world: &World,
    tiles: &[WorldTileCoords],
    gpu_vector: &impl HasTile,
    painted_tiles: &mut Vec<WorldTileCoords>,
) {
    let mut seen_painted: FxHashSet<_> = painted_tiles.iter().copied().collect();
    for &coords in tiles {
        if gpu_vector.has_tile(coords, world) {
            if seen_painted.insert(coords) {
                painted_tiles.push(coords);
            }
            continue;
        }
        for stand_in in loaded_cover(gpu_vector, coords, world)
            .or_else(|| {
                gpu_vector
                    .get_available_children(coords, world, CHILDREN_SEARCH_DEPTH)
                    .filter(|children| !children.is_empty())
            })
            .into_iter()
            .flatten()
        {
            if seen_painted.insert(stand_in) {
                painted_tiles.push(stand_in);
            }
        }
    }
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
        let is_uncovered = terrain
            .is_some_and(|t| t.drape_texture(target).is_none() && !has_present_ancestor(target, t));
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

#[cfg(test)]
mod tests;
