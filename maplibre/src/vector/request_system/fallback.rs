//! Pan-hole prevention fallbacks for non-terrain vector views: coarse `z - 3` ancestor cover,
//! active `PrefetchView` pan lookahead, and settled 1-ring edge prefetch within `MemoryBudget`.

use rustc_hash::FxHashSet;

use crate::{
    coords::{OverscaledTileId, WorldTileCoords, ZoomLevel},
    io::tile_sources::{
        clamp_to_max_zoom, outside_source_bounds, source_max_zoom, source_min_zoom, TileKind,
    },
    render::{
        eventually::Eventually,
        memory_budget::MemoryBudget,
        projection::view_region_for_projection,
        tile_view_pattern::DEFAULT_TILE_SIZE,
        view_state::{ViewState, ViewStatePadding},
    },
    style::Style,
    tcs::world::World,
    vector::{
        upload_system::sources::VectorFallbackTiles, VectorBufferPool, VectorLayerBucketComponent,
    },
};

/// Maximum coarse `z - 3` ancestor cover tiles requested alongside the tight viewport during motion.
const MAX_COARSE_ANCESTORS: usize = 2;
/// Maximum `PrefetchView` pan lookahead tiles requested on `wasm32` during camera motion.
const MAX_PAN_LOOKAHEAD_TILES: usize = 6;

/// Computes extra non-terrain fallback/prefetch tiles (`wasm32` coarse cover and pan lookahead)
/// and records all non-tight retained canonical coordinates into [`VectorFallbackTiles`] so
/// `upload_system` uploads and keeps them in `VectorBufferPool`.
pub(super) fn plan_non_terrain_fallbacks(
    style: &Style,
    view_state: &ViewState,
    world: &mut World,
    (tight_requests, is_multiview, tight_covering): (bool, bool, bool),
    view_tiles: &[WorldTileCoords],
    (level, magnifies): (u8, bool),
) -> Vec<WorldTileCoords> {
    plan_non_terrain_fallbacks_for_platform(
        style,
        view_state,
        world,
        (tight_requests, is_multiview, tight_covering),
        view_tiles,
        (level, magnifies),
        cfg!(target_arch = "wasm32"),
    )
}

fn plan_non_terrain_fallbacks_for_platform(
    style: &Style,
    view_state: &ViewState,
    world: &mut World,
    (tight_requests, is_multiview, tight_covering): (bool, bool, bool),
    view_tiles: &[WorldTileCoords],
    (level, magnifies): (u8, bool),
    is_web: bool,
) -> Vec<WorldTileCoords> {
    if style.terrain.is_some() || tight_requests || is_multiview || view_tiles.is_empty() {
        world.resources.insert(VectorFallbackTiles::default());
        return Vec::new();
    }
    let memory = world
        .resources
        .get::<MemoryBudget>()
        .copied()
        .unwrap_or_default();
    if memory.is_tight() {
        world.resources.insert(VectorFallbackTiles::default());
        return Vec::new();
    }

    let visible_level = ZoomLevel::new(level);
    let mut extra_requests = Vec::new();
    let max_zoom = source_max_zoom(style, TileKind::Vector);
    let min_zoom = source_min_zoom(style, TileKind::Vector);
    let mut handoff_parents = Vec::new();

    if is_web && !view_state.has_external_view() {
        let source_z = max_zoom.map_or(level, |m| level.min(m));
        let ahead_view = world
            .resources
            .get::<crate::render::xr::PrefetchView>()
            .and_then(|pv| pv.view_state.clone());
        if source_z < 3 && ahead_view.is_none() {
            if tight_covering {
                if world
                    .resources
                    .get::<VectorFallbackTiles>()
                    .is_some_and(|f| !f.0.is_empty())
                {
                    world.resources.insert(VectorFallbackTiles::default());
                }
                return extra_requests;
            }
        } else {
            let allow_new_coarse = super::velocity::should_request_coarse_fallback(
                world,
                view_state,
                ahead_view.is_some(),
            );
            fn ensure_seen<'a>(
                view_set: &'a mut Option<FxHashSet<WorldTileCoords>>,
                view_tiles: &[WorldTileCoords],
            ) -> &'a mut FxHashSet<WorldTileCoords> {
                view_set.get_or_insert_with(|| view_tiles.iter().copied().collect())
            }
            let mut view_set: Option<FxHashSet<WorldTileCoords>> = None;

            // 1. Coarse `z - 3` ancestor cover (covers an 8x8 tile block, up to `MAX_COARSE_ANCESTORS`
            // across seams): prioritized for uncovered ancestors, keeping `coarse_z - 1` alive during
            // zoom handoffs until the new `coarse_z` finishes uploading to the GPU.
            if source_z >= 3 {
                let coarse_z = source_z - 3;
                let all_children_uploaded = |world: &World, ancestor: WorldTileCoords| {
                    let Some(Eventually::Initialized(pool)) =
                        world.resources.get::<Eventually<VectorBufferPool>>()
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
                        if source_child.x >> shift == ancestor.x
                            && source_child.y >> shift == ancestor.y
                        {
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
                };
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
                    let uncovered = !all_children_uploaded(world, ancestor);
                    if !spawned && (!allow_new_coarse || !uncovered) {
                        continue;
                    }
                    candidates.push((ancestor, uncovered));
                }
                candidates.sort_by_key(|(_, uncovered)| !*uncovered);
                let selected_ancestors: Vec<_> = candidates
                    .into_iter()
                    .map(|(ancestor, _)| ancestor)
                    .take(MAX_COARSE_ANCESTORS)
                    .collect();
                let seen_extra = ensure_seen(&mut view_set, view_tiles);
                for ancestor in selected_ancestors {
                    if seen_extra.insert(ancestor) {
                        extra_requests.push(ancestor);
                    }
                    if !tile_spawned(world, ancestor)
                        || !crate::vector::geometry_uploaded(ancestor, world)
                    {
                        if let Some(parent) = ancestor.get_parent() {
                            if min_zoom.is_none_or(|m| u8::from(parent.z) >= m)
                                && tile_spawned(world, parent)
                            {
                                handoff_parents.push(parent);
                            }
                        }
                    }
                }
            }

            // 2. Active pan lookahead (`PrefetchView`) while the camera is moving.
            // Settled 1-ring edge tiles (`MAX_SETTLED_RING_TILES = 0`) are intentionally omitted on web
            // to eliminate second-wave boot fetches and save +25-80 MiB of off-screen urban tiles.
            if let Some(ahead) = ahead_view {
                if let Ok(Some(region)) = view_region_for_projection(
                    style,
                    &ahead,
                    world,
                    visible_level,
                    ViewStatePadding::Tight,
                ) {
                    let seen_extra = ensure_seen(&mut view_set, view_tiles);
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
            }
        }
    }

    // Record canonical source coordinates for all retained non-tight tiles (both native `Loose`
    // ring tiles already in `view_tiles` and `wasm32` `extra_requests`) so `upload_system`
    // uploads and retains them in `VectorBufferPool`.
    if tight_covering && extra_requests.is_empty() && handoff_parents.is_empty() {
        if world
            .resources
            .get::<VectorFallbackTiles>()
            .is_none_or(|f| !f.0.is_empty())
        {
            world.resources.insert(VectorFallbackTiles::default());
        }
        return extra_requests;
    }
    let non_tight: Vec<WorldTileCoords> = if tight_covering {
        Vec::new()
    } else {
        let tight_set: FxHashSet<WorldTileCoords> = view_region_for_projection(
            style,
            view_state,
            world,
            view_state.zoom().zoom_level(DEFAULT_TILE_SIZE),
            ViewStatePadding::Tight,
        )
        .ok()
        .flatten()
        .map(|r| r.iter().collect())
        .unwrap_or_default();
        view_tiles
            .iter()
            .copied()
            .filter(|c| !tight_set.contains(c))
            .collect()
    };

    let mut fallback_canonical = Vec::new();
    let mut seen_canonical = FxHashSet::default();
    for &coords in non_tight
        .iter()
        .chain(extra_requests.iter())
        .chain(handoff_parents.iter())
    {
        if min_zoom.is_some_and(|m| u8::from(coords.z) < m) {
            continue;
        }
        let target_level = u8::from(coords.z).min(level);
        let Some(target) = OverscaledTileId::for_source(coords, target_level, max_zoom, magnifies)
        else {
            continue;
        };
        let canonical = target.canonical;
        if !outside_source_bounds(style, TileKind::Vector, canonical)
            && seen_canonical.insert(canonical)
        {
            fallback_canonical.push(canonical);
        }
    }
    world
        .resources
        .insert(VectorFallbackTiles(fallback_canonical));

    extra_requests
}

fn tile_spawned(world: &World, coords: WorldTileCoords) -> bool {
    coords
        .build_quad_key()
        .is_some_and(|qk| world.tiles.tiles.contains_key(&qk))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        coords::{LatLon, WorldCoords, Zoom},
        window::PhysicalSize,
    };

    #[test]
    fn stationary_web_cold_boot_requests_zero_extra_tiles_and_pan_requests_coarse_cover() {
        let style: Style = serde_json::from_str(
            r#"{
                "version": 8,
                "sources": { "v": { "type": "vector", "tiles": ["https://example.invalid/{z}/{x}/{y}.pbf"], "maxzoom": 14 } },
                "layers": []
            }"#,
        )
        .expect("valid style");
        let mut world = World::default();
        let size = PhysicalSize::new(800, 600).expect("valid");
        let zoom = Zoom::new(14.0);
        let view_state = ViewState::new(
            size,
            WorldCoords::from_lat_lon(LatLon::new(52.3702, 4.8952), zoom),
            zoom,
            cgmath::Deg(0.0),
            cgmath::Deg(45.0),
        );
        let tile = WorldTileCoords {
            x: 8411,
            y: 5386,
            z: ZoomLevel::new(14),
        };
        assert!(!crate::render::frame_signals::camera_moved(
            &world,
            &view_state
        ));
        let stationary = plan_non_terrain_fallbacks_for_platform(
            &style,
            &view_state,
            &mut world,
            (false, false, true),
            &[tile],
            (14, false),
            true,
        );
        assert!(stationary.is_empty());

        let ahead = ViewState::new(
            size,
            WorldCoords::from_lat_lon(LatLon::new(52.3712, 4.8962), zoom),
            zoom,
            cgmath::Deg(0.0),
            cgmath::Deg(45.0),
        );
        world.resources.insert(crate::render::xr::PrefetchView {
            view_state: Some(ahead),
            placement: None,
        });
        let moving = plan_non_terrain_fallbacks_for_platform(
            &style,
            &view_state,
            &mut world,
            (false, false, true),
            &[tile],
            (14, false),
            true,
        );
        assert!(moving.contains(&WorldTileCoords {
            x: 8411 >> 3,
            y: 5386 >> 3,
            z: ZoomLevel::new(11),
        }));
    }

    #[test]
    fn coarse_fallback_covers_two_seam_ancestors_and_retains_parent_during_handoff() {
        let style: Style = serde_json::from_str(
            r#"{
                "version": 8,
                "sources": { "v": { "type": "vector", "tiles": ["https://example.invalid/{z}/{x}/{y}.pbf"], "maxzoom": 14 } },
                "layers": []
            }"#,
        )
        .expect("valid style");
        let mut world = World::default();
        let size = PhysicalSize::new(800, 600).expect("valid");
        let zoom = Zoom::new(14.5);
        let view_state = ViewState::new(
            size,
            WorldCoords::from_lat_lon(LatLon::new(35.6812, 139.75), zoom),
            zoom,
            cgmath::Deg(0.0),
            cgmath::Deg(0.0),
        );
        // Two z14 tiles crossing a z11 seam: x=14543 (z11 x=1817) and x=14544 (z11 x=1818).
        let left_z14 = WorldTileCoords {
            x: 1817 << 3 | 7,
            y: 6450,
            z: ZoomLevel::new(14),
        };
        let right_z14 = WorldTileCoords {
            x: 1818 << 3,
            y: 6450,
            z: ZoomLevel::new(14),
        };
        let parent_z10 = WorldTileCoords {
            x: 1817 >> 1,
            y: (6450 >> 3) >> 1,
            z: ZoomLevel::new(10),
        };
        world.tiles.spawn_mut(parent_z10).expect("valid parent");
        let ahead = ViewState::new(
            size,
            WorldCoords::from_lat_lon(LatLon::new(35.6812, 139.74), zoom),
            zoom,
            cgmath::Deg(0.0),
            cgmath::Deg(0.0),
        );
        world.resources.insert(crate::render::xr::PrefetchView {
            view_state: Some(ahead),
            placement: None,
        });

        let extra = plan_non_terrain_fallbacks_for_platform(
            &style,
            &view_state,
            &mut world,
            (false, false, true),
            &[left_z14, right_z14],
            (14, false),
            true,
        );
        assert_eq!(extra.len(), 2, "seam requires 2 coarse z11 ancestors");
        let retained = world
            .resources
            .get::<VectorFallbackTiles>()
            .expect("fallbacks");
        assert!(
            retained.0.contains(&parent_z10),
            "spawned z10 parent stays retained while z11 coarse ancestor has not uploaded"
        );
        assert!(!extra.contains(&parent_z10));
    }
}
