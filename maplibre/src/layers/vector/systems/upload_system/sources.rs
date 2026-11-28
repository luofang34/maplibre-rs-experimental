//! Geometry uploads follow the actual sources selected for the view or terrain textures,
//! prioritizing uncovered visible tiles first and retaining coarse overview / 1-ring stand-ins.
use std::sync::Arc;

use rustc_hash::FxHashSet;

use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    io::tile_sources::TileKind,
    render::{
        projection::view_region_for_projection,
        tile_view_pattern::{
            coverage::loaded_cover, HasTile, ViewTileSources, CHILDREN_SEARCH_DEPTH,
            DEFAULT_TILE_SIZE,
        },
        view_state::{ViewState, ViewStatePadding},
    },
    schedule::system::SystemError,
    style::Style,
    tcs::world::World,
    vector::request_system::VectorFallbackTiles,
};

/// Resolved source upload order and painted/retained tile sets for one vector upload pass.
#[derive(Clone)]
pub(super) struct FrameUploadSources {
    pub(super) source_tiles: Arc<[WorldTileCoords]>,
    pub(super) painted_tiles: Arc<[WorldTileCoords]>,
    pub(super) active_painted_tiles: Arc<[WorldTileCoords]>,
    pub(super) uncovered: Arc<FxHashSet<WorldTileCoords>>,
}

mod cache;

use cache::{
    build_frame_sources_key, terrain_sources, update_loose_tight_tiles, FrameSourcesCache,
};

type FreshTightTiles = (ZoomLevel, Arc<[WorldTileCoords]>);

#[cfg(all(test, feature = "headless", feature = "thread-safe-futures"))]
pub(super) fn frame_sources_and_painted(
    world: &World,
    style: &Style,
    view_state: &ViewState,
) -> Result<
    (
        Arc<[WorldTileCoords]>,
        Arc<[WorldTileCoords]>,
        Arc<FxHashSet<WorldTileCoords>>,
    ),
    SystemError,
> {
    let resolved = frame_upload_sources(world, style, view_state)?;
    Ok((
        resolved.source_tiles,
        resolved.painted_tiles,
        resolved.uncovered,
    ))
}

#[cfg(all(test, feature = "headless", feature = "thread-safe-futures"))]
pub(super) fn frame_upload_sources(
    world: &World,
    style: &Style,
    view_state: &ViewState,
) -> Result<FrameUploadSources, SystemError> {
    let mut scratch = Vec::new();
    let needs_view_tiles = compute_needs_view_tiles_for_paint(style);
    let (present, fresh_tight) =
        resolve_view_tiles_for_paint(world, style, view_state, needs_view_tiles, &mut scratch)?;
    let _ = fresh_tight;
    compute_frame_sources_with_tiles(world, style, present.then_some(scratch.as_slice()))
}

pub(super) fn frame_sources_and_painted_mut(
    world: &mut World,
    style: &Style,
    view_state: &ViewState,
) -> Result<FrameUploadSources, SystemError> {
    let style_rev = style.revision();
    let (mut scratch, needs_view_tiles) = match world.resources.get_mut::<FrameSourcesCache>() {
        Some(cache) => {
            let needs = match cache.needs_view_tiles_for_paint {
                Some((rev, needs)) if rev == style_rev => needs,
                _ => {
                    let needs = compute_needs_view_tiles_for_paint(style);
                    cache.needs_view_tiles_for_paint = Some((style_rev, needs));
                    needs
                }
            };
            (std::mem::take(&mut cache.view_tiles_scratch), needs)
        }
        None => (Vec::new(), compute_needs_view_tiles_for_paint(style)),
    };
    let (present, fresh_tight) =
        resolve_view_tiles_for_paint(world, style, view_state, needs_view_tiles, &mut scratch)?;
    update_loose_tight_tiles(world, fresh_tight.as_ref());
    let resolved_tiles = present.then_some(scratch.as_slice());
    let key = build_frame_sources_key(world, style, view_state, resolved_tiles);
    if let Some(ref key) = key
        && key.terrain_settled
        && let Some(hit) = world
            .resources
            .get::<FrameSourcesCache>()
            .filter(|cache| cache.key.as_ref() == Some(key))
            .and_then(|cache| cache.result.clone())
    {
        if let Some(cache) = world.resources.get_mut::<FrameSourcesCache>() {
            cache.view_tiles_scratch = scratch;
        }
        return Ok(hit);
    }
    let computed = compute_frame_sources_with_tiles(world, style, resolved_tiles);
    let cache = world.resources.get_or_init_mut::<FrameSourcesCache>();
    cache.view_tiles_scratch = scratch;
    let result = computed?;
    if key.is_some() {
        cache.key = key;
        cache.result = Some(result.clone());
    }
    Ok(result)
}

fn compute_frame_sources_with_tiles(
    world: &World,
    style: &Style,
    view_tiles: Option<&[WorldTileCoords]>,
) -> Result<FrameUploadSources, SystemError> {
    let Some(view_tile_sources) = world.resources.get::<ViewTileSources>() else {
        return Err(SystemError::Dependencies);
    };
    let cpu_vector = view_tile_sources.of_kind(TileKind::Vector);
    let gpu_vector = (
        view_tile_sources.of_kind(TileKind::Vector),
        crate::vector::UploadedVectorGeometry::for_world(world),
    );
    if style.terrain.is_some() {
        let (src, uncov) = terrain_sources(world, style);
        let mut painted = src.clone();
        if let Some(tiles) = view_tiles {
            append_missing_gpu_stand_ins(world, tiles, &gpu_vector, &mut painted);
        }
        let painted_arc: Arc<[WorldTileCoords]> = Arc::from(painted);
        return Ok(FrameUploadSources {
            source_tiles: Arc::from(src),
            painted_tiles: Arc::clone(&painted_arc),
            active_painted_tiles: painted_arc,
            uncovered: Arc::new(uncov),
        });
    }
    let (source_tiles, mut active_painted, retained_only, uncovered, all_view_painted) =
        partition_non_terrain_sources(world, view_tiles, &cpu_vector, &gpu_vector);
    if !all_view_painted {
        if let Some(tiles) = view_tiles {
            append_missing_gpu_stand_ins(world, tiles, &gpu_vector, &mut active_painted);
        }
    }
    let active_arc: Arc<[WorldTileCoords]> = Arc::from(active_painted);
    let painted_arc = if retained_only.is_empty() {
        Arc::clone(&active_arc)
    } else {
        let mut painted = Vec::with_capacity(active_arc.len() + retained_only.len());
        painted.extend_from_slice(&active_arc);
        for c in retained_only {
            if !active_arc.contains(&c) {
                painted.push(c);
            }
        }
        Arc::from(painted)
    };
    Ok(FrameUploadSources {
        source_tiles: Arc::from(source_tiles),
        painted_tiles: painted_arc,
        active_painted_tiles: active_arc,
        uncovered: Arc::new(uncovered),
    })
}

fn resolve_cached_wrapped_tiles(
    world: &World,
    style: &Style,
    view_state: &ViewState,
    visible_level: ZoomLevel,
    multi_active: bool,
    out: &mut Vec<WorldTileCoords>,
) -> Option<bool> {
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
            .map(|(tiles, _, _)| tiles.as_deref())
        {
            out.clear();
            return Some(if let Some(tiles) = tiles {
                out.extend(
                    tiles
                        .iter()
                        .filter_map(|coords| coords.wrapped().map(|(wrapped, _)| wrapped)),
                );
                true
            } else {
                false
            });
        }
    }
    crate::render::eye_covering::matching_tight_tiles(world, style, view_state, visible_level).map(
        |shared_tiles| {
            out.clear();
            out.extend(
                shared_tiles
                    .iter()
                    .filter_map(|coords| coords.wrapped().map(|(w, _)| w)),
            );
            true
        },
    )
}

fn compute_needs_view_tiles_for_paint(style: &Style) -> bool {
    style.terrain.is_none()
        || style.layers.iter().any(|l| {
            !l.is_hidden()
                && !matches!(
                    l.type_.as_str(),
                    "background" | "symbol" | "sky" | "raster" | "hillshade" | "color-relief"
                )
                && (!crate::terrain::is_drapeable(&l.type_)
                    || crate::vector::structures::kind(l).is_some())
        })
}

fn resolve_view_tiles_for_paint(
    world: &World,
    style: &Style,
    view_state: &ViewState,
    needs_view_tiles_for_paint: bool,
    out: &mut Vec<WorldTileCoords>,
) -> Result<(bool, Option<FreshTightTiles>), SystemError> {
    if !needs_view_tiles_for_paint {
        out.clear();
        return Ok((false, None));
    }
    let visible_level = view_state.zoom().zoom_level(DEFAULT_TILE_SIZE);
    let multi_active = world
        .resources
        .get::<crate::render::eye_covering::MultiViewExtractCovering>()
        .is_some_and(|m| m.active);
    if let Some(present) =
        resolve_cached_wrapped_tiles(world, style, view_state, visible_level, multi_active, out)
    {
        return Ok((present, None));
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
    out.clear();
    let present = if let Some(ref r) = region {
        out.extend(
            r.copies()
                .filter_map(|copy| copy.wrapped().map(|(coords, _)| coords)),
        );
        true
    } else {
        false
    };
    let fresh = if !multi_active {
        region
            .as_ref()
            .map(|r| (visible_level, Arc::from(r.copies().collect::<Vec<_>>())))
    } else {
        None
    };
    Ok((present, fresh))
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

type NonTerrainPartitionResult = (
    Vec<WorldTileCoords>,
    Vec<WorldTileCoords>,
    Vec<WorldTileCoords>,
    FxHashSet<WorldTileCoords>,
    bool,
);

fn partition_non_terrain_sources(
    world: &World,
    view_tiles: Option<&[WorldTileCoords]>,
    cpu_vector: &impl HasTile,
    gpu_vector: &impl HasTile,
) -> NonTerrainPartitionResult {
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
    let (coarse_uncovered, retained_only) =
        collect_fallback_sources(world, cpu_vector, gpu_vector, &mut part);
    let mut active_painted = Vec::with_capacity(
        coarse_uncovered.len() + part.uncovered_list.len() + part.covered_list.len(),
    );
    active_painted.extend_from_slice(&coarse_uncovered);
    active_painted.extend_from_slice(&part.uncovered_list);
    active_painted.extend_from_slice(&part.covered_list);
    let mut upload_order = coarse_uncovered;
    upload_order.extend(part.uncovered_list);
    upload_order.extend(part.covered_list);
    upload_order.extend_from_slice(&retained_only);
    (
        upload_order,
        active_painted,
        retained_only,
        part.uncovered_set,
        all_view_painted,
    )
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
) -> (Vec<WorldTileCoords>, Vec<WorldTileCoords>) {
    let mut coarse_uncovered = Vec::new();
    let mut retained_only = Vec::new();
    let Some(fallbacks) = world.resources.get::<VectorFallbackTiles>() else {
        return (coarse_uncovered, retained_only);
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
            retained_only.push(coords);
        }
    }
    (coarse_uncovered, retained_only)
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

#[cfg(test)]
mod tests;
