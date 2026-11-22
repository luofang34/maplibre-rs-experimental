use super::*;

pub(super) struct WebFallbackPlan {
    pub extra_requests: Vec<WorldTileCoords>,
    pub handoff_parents: Vec<WorldTileCoords>,
    pub skip_empty: bool,
}

pub(super) fn plan_web_fallbacks(
    style: &Style,
    view_state: &ViewState,
    world: &mut World,
    view_tiles: &[WorldTileCoords],
    (level, max_zoom, min_zoom, tight_covering): (u8, Option<u8>, Option<u8>, bool),
) -> WebFallbackPlan {
    let mut extra_requests = Vec::new();
    let mut handoff_parents = Vec::new();
    let source_z = max_zoom.map_or(level, |m| level.min(m));
    let ahead_view = world
        .resources
        .get::<crate::render::xr::PrefetchView>()
        .and_then(|pv| pv.view_state.clone());
    if source_z < 3 && ahead_view.is_none() {
        return WebFallbackPlan {
            extra_requests,
            handoff_parents,
            skip_empty: tight_covering,
        };
    }
    let allow_new_coarse = super::super::velocity::should_request_coarse_fallback(
        world,
        view_state,
        ahead_view.is_some(),
    );
    let mut view_set: Option<FxHashSet<WorldTileCoords>> = None;
    if source_z >= 3 {
        collect_coarse_ancestors(
            world,
            view_tiles,
            (source_z - 3, max_zoom, min_zoom, allow_new_coarse),
            &mut view_set,
            &mut extra_requests,
            &mut handoff_parents,
        );
    }
    if let Some(ahead) = ahead_view {
        collect_pan_lookahead(
            style,
            &ahead,
            world,
            ZoomLevel::new(level),
            view_tiles,
            &mut view_set,
            &mut extra_requests,
        );
    }
    WebFallbackPlan {
        extra_requests,
        handoff_parents,
        skip_empty: false,
    }
}

fn ensure_seen<'a>(
    view_set: &'a mut Option<FxHashSet<WorldTileCoords>>,
    view_tiles: &[WorldTileCoords],
) -> &'a mut FxHashSet<WorldTileCoords> {
    view_set.get_or_insert_with(|| view_tiles.iter().copied().collect())
}

fn collect_coarse_ancestors(
    world: &World,
    view_tiles: &[WorldTileCoords],
    (coarse_z, max_zoom, min_zoom, allow_new_coarse): (u8, Option<u8>, Option<u8>, bool),
    view_set: &mut Option<FxHashSet<WorldTileCoords>>,
    extra_requests: &mut Vec<WorldTileCoords>,
    handoff_parents: &mut Vec<WorldTileCoords>,
) {
    let mut candidates = Vec::new();
    let mut seen_ancestors = FxHashSet::default();
    for &coords in view_tiles {
        let Some((wrapped, _)) = coords.wrapped() else {
            continue;
        };
        let cur_z = u8::from(wrapped.z);
        if cur_z <= coarse_z {
            continue;
        }
        let shift = u32::from(cur_z - coarse_z);
        let ancestor = WorldTileCoords {
            x: wrapped.x >> shift,
            y: wrapped.y >> shift,
            z: ZoomLevel::new(coarse_z),
        };
        if !seen_ancestors.insert(ancestor) {
            continue;
        }
        let spawned = tile_spawned(world, ancestor);
        let uncovered = !all_children_uploaded(world, view_tiles, max_zoom, ancestor);
        if spawned || (allow_new_coarse && uncovered) {
            candidates.push((ancestor, uncovered));
        }
    }
    candidates.sort_by_key(|(_, uncovered)| !*uncovered);
    let seen_extra = ensure_seen(view_set, view_tiles);
    for (ancestor, _) in candidates.into_iter().take(MAX_COARSE_ANCESTORS) {
        if seen_extra.insert(ancestor) {
            extra_requests.push(ancestor);
        }
        if !tile_spawned(world, ancestor) || !crate::vector::geometry_uploaded(ancestor, world) {
            if let Some(parent) = ancestor.get_parent() {
                if min_zoom.is_none_or(|m| u8::from(parent.z) >= m) && tile_spawned(world, parent) {
                    handoff_parents.push(parent);
                }
            }
        }
    }
}

fn all_children_uploaded(
    world: &World,
    view_tiles: &[WorldTileCoords],
    max_zoom: Option<u8>,
    ancestor: WorldTileCoords,
) -> bool {
    let Some(Eventually::Initialized(pool)) = world.resources.get::<Eventually<VectorBufferPool>>()
    else {
        return false;
    };
    if !pool.has_capacity() {
        return false;
    }
    let az = u8::from(ancestor.z);
    let mut any_covered = false;
    for &child in view_tiles {
        let Some((wrapped, _)) = child.wrapped() else {
            continue;
        };
        let source_child = clamp_to_max_zoom(wrapped, max_zoom);
        let cz = u8::from(source_child.z);
        if cz <= az {
            continue;
        }
        let shift = u32::from(cz - az);
        if source_child.x >> shift == ancestor.x && source_child.y >> shift == ancestor.y {
            any_covered = true;
            if world
                .tiles
                .query::<&VectorLayerBucketComponent>(source_child)
                .is_none_or(|b| !b.done)
                || !crate::vector::geometry_uploaded(source_child, world)
            {
                return false;
            }
        }
    }
    any_covered
}

fn collect_pan_lookahead(
    style: &Style,
    ahead: &ViewState,
    world: &World,
    visible_level: ZoomLevel,
    view_tiles: &[WorldTileCoords],
    view_set: &mut Option<FxHashSet<WorldTileCoords>>,
    extra_requests: &mut Vec<WorldTileCoords>,
) {
    let Ok(Some(region)) =
        view_region_for_projection(style, ahead, world, visible_level, ViewStatePadding::Tight)
    else {
        return;
    };
    let seen_extra = ensure_seen(view_set, view_tiles);
    let mut lookahead_count = 0;
    for coords in region.iter() {
        if seen_extra.insert(coords) {
            extra_requests.push(coords);
            lookahead_count += 1;
            if lookahead_count >= MAX_PAN_LOOKAHEAD_TILES {
                break;
            }
        }
    }
}
