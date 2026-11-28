//! Stage systems for GPU upload/projection, phase sorting, render graph execution, and cleanup.

use std::borrow::Cow;

use crate::{
    context::MapContext,
    coords::WorldTileCoords,
    render::{
        error::RenderError,
        eventually::{Eventually, Eventually::Initialized},
        graph_runner::run_graph,
        projection::ProjectionGpuResources,
        render_phase::{LayerItem, RenderPhase, TileMaskItem, TranslucentItem},
        resolved_view::ResolvedFrameView,
        tile_mesh::{GlobeTileMeshCache, TileMeshUsage},
        tile_view_pattern::WgpuTileViewPattern,
        Renderer,
    },
    schedule::system::{System, SystemError, SystemResult},
    tcs::world::World,
};

#[derive(Clone, Default)]
struct GlobeMeshPrepareMemo {
    flat_prepared: bool,
    flat_with_raster: bool,
    needs_raster_mesh: Option<(u64, bool)>,
    last_globe: Option<(u64, bool, bool, Vec<WorldTileCoords>)>,
}

fn tile_sets_match(drawn: &[WorldTileCoords], cached_bg: &[WorldTileCoords]) -> bool {
    if !drawn.is_empty()
        && drawn == cached_bg
        && drawn.iter().all(|coords| coords.wrapped().is_some())
    {
        return true;
    }
    let mut wrapped = drawn
        .iter()
        .copied()
        .filter(|coords| coords.wrapped().is_some());
    let Some(first) = wrapped.next() else {
        return cached_bg == [WorldTileCoords::default()];
    };
    cached_bg.contains(&first)
        && wrapped.all(|coords| cached_bg.contains(&coords))
        && cached_bg
            .iter()
            .all(|coords| drawn.iter().any(|d| d == coords && d.wrapped().is_some()))
}

fn globe_tiles_match(world: &World, cached_bg: &[WorldTileCoords]) -> bool {
    let drawn = crate::render::eye_covering::drawn_tiles(world).unwrap_or_default();
    tile_sets_match(drawn, cached_bg)
}

pub(crate) fn upload_system(
    MapContext {
        world,
        style,
        view_state,
        renderer: Renderer { device, queue, .. },
        ..
    }: &mut MapContext,
) -> SystemResult {
    let (view_proj, projection_data) = match world
        .resources
        .get::<ResolvedFrameView>()
        .filter(|cached| cached.matches(style, view_state))
    {
        Some(cached) => (cached.gpu_view_projection, cached.shader_projection),
        None => {
            let resolved = ResolvedFrameView::resolve(style, view_state).map_err(|error| {
                tracing::error!(%error, "unable to prepare projection state");
                SystemError::Setup
            })?;
            let snapshot = (resolved.gpu_view_projection, resolved.shader_projection);
            if let Some(slot) = world.resources.get_mut::<ResolvedFrameView>() {
                *slot = resolved;
            } else {
                world.resources.insert(resolved);
            }
            snapshot
        }
    };
    let style_rev = style.revision();
    let uses_globe = projection_data.transition > 0.0;
    if !world.resources.exists::<GlobeMeshPrepareMemo>() {
        world.resources.insert(GlobeMeshPrepareMemo::default());
    }

    let (topology_gen, needs_raster_mesh, needs_stencil_borders, meshes_empty) = {
        let Some((
            Initialized(tile_view_pattern),
            Initialized(projection_resources),
            tile_mesh_cache,
            memo,
        )) = world.resources.query_mut::<(
            &mut Eventually<WgpuTileViewPattern>,
            &mut Eventually<ProjectionGpuResources>,
            &mut GlobeTileMeshCache,
            &mut GlobeMeshPrepareMemo,
        )>()
        else {
            return Err(SystemError::Dependencies);
        };

        let needs_raster_mesh =
            if let Some((_, needs)) = memo.needs_raster_mesh.filter(|(rev, _)| *rev == style_rev) {
                needs
            } else {
                style.layers.iter().any(|layer| {
                    !layer.is_hidden()
                        && matches!(
                            layer.type_.as_str(),
                            "raster" | "hillshade" | "color-relief"
                        )
                })
            };

        tile_view_pattern.upload_pattern_with_projection(
            queue,
            &view_proj,
            view_state.width() as f32,
            view_state.height() as f32,
            view_state.style_zoom(),
            projection_data.transition >= 1.0,
        );
        projection_resources.upload(queue, projection_data);
        tile_mesh_cache.set_globe_active(uses_globe);
        let (min_z, max_z) = tile_view_pattern.all_zoom_bounds();
        let borders = needs_raster_mesh
            || crate::render::projection::needs_vector_stencil_borders(
                projection_data.transition,
                view_state.zoom().value(),
                view_state.has_external_view(),
                min_z,
                max_z,
            );
        let empty = tile_mesh_cache.is_empty();
        if empty {
            *memo = GlobeMeshPrepareMemo::default();
        }
        memo.needs_raster_mesh = Some((style_rev, needs_raster_mesh));
        if !uses_globe {
            memo.last_globe = None;
            if !(memo.flat_prepared && (!needs_raster_mesh || memo.flat_with_raster)) {
                let coords = WorldTileCoords::default();
                let flat_usages: &[(TileMeshUsage, bool)] = if needs_raster_mesh {
                    &[
                        (TileMeshUsage::Stencil, false),
                        (TileMeshUsage::Raster, true),
                        (TileMeshUsage::Raster, false),
                    ]
                } else {
                    &[(TileMeshUsage::Stencil, false)]
                };
                for &(usage, generate_borders) in flat_usages {
                    tile_mesh_cache
                        .prepare(device, queue, coords, usage, generate_borders)
                        .map_err(|error| {
                            tracing::error!(%error, %coords, "unable to prepare globe tile mesh");
                            SystemError::Setup
                        })?;
                }
                memo.flat_prepared = true;
                memo.flat_with_raster |= needs_raster_mesh;
            }
            return Ok(());
        }
        (
            tile_view_pattern.topology_generation(),
            needs_raster_mesh,
            borders,
            empty,
        )
    };
    let _ = meshes_empty;

    let globe_unchanged = world
        .resources
        .get::<GlobeMeshPrepareMemo>()
        .and_then(|memo| memo.last_globe.as_ref())
        .is_some_and(|(tg, raster, borders, bg)| {
            *tg == topology_gen
                && (*raster || !needs_raster_mesh)
                && (*borders || !needs_stencil_borders)
                && globe_tiles_match(world, bg)
        });
    if globe_unchanged {
        return Ok(());
    }

    let visible_tiles: Vec<_> = {
        let Some(Initialized(tile_view_pattern)) =
            world.resources.get::<Eventually<WgpuTileViewPattern>>()
        else {
            return Err(SystemError::Dependencies);
        };
        let mut tiles = Vec::new();
        for view_tile in tile_view_pattern.iter() {
            view_tile.render(|shape| tiles.push(shape.coords()));
        }
        tiles
    };

    let usages: &[(TileMeshUsage, bool)] = match (needs_raster_mesh, needs_stencil_borders) {
        (true, true) => &[
            (TileMeshUsage::Stencil, true),
            (TileMeshUsage::Stencil, false),
            (TileMeshUsage::Raster, true),
            (TileMeshUsage::Raster, false),
        ],
        (true, false) => &[
            (TileMeshUsage::Stencil, false),
            (TileMeshUsage::Raster, true),
            (TileMeshUsage::Raster, false),
        ],
        (false, true) => &[
            (TileMeshUsage::Stencil, true),
            (TileMeshUsage::Stencil, false),
        ],
        (false, false) => &[(TileMeshUsage::Stencil, false)],
    };

    let background_tiles = crate::render::eye_covering::globe_tiles(world);
    let Some((tile_mesh_cache, memo)) = world
        .resources
        .query_mut::<(&mut GlobeTileMeshCache, &mut GlobeMeshPrepareMemo)>()
    else {
        return Err(SystemError::Dependencies);
    };
    tile_mesh_cache.advance_epoch();
    for &coords in &background_tiles {
        tile_mesh_cache
            .prepare(device, queue, coords, TileMeshUsage::Raster, false)
            .map_err(|error| {
                tracing::error!(%error, %coords, "unable to prepare globe background mesh");
                SystemError::Setup
            })?;
    }
    for coords in visible_tiles {
        for &(usage, generate_borders) in usages {
            tile_mesh_cache
                .prepare(device, queue, coords, usage, generate_borders)
                .map_err(|error| {
                    tracing::error!(%error, %coords, "unable to prepare globe tile mesh");
                    SystemError::Setup
                })?;
        }
    }
    tile_mesh_cache.evict_stale(32);
    let (with_raster, with_borders) = {
        let prev = memo.last_globe.as_ref();
        (
            prev.is_some_and(|(tg, raster, _, _)| *tg == topology_gen && *raster)
                || needs_raster_mesh,
            prev.is_some_and(|(tg, _, borders, _)| *tg == topology_gen && *borders)
                || needs_stencil_borders,
        )
    };
    memo.last_globe = Some((topology_gen, with_raster, with_borders, background_tiles));

    Ok(())
}

/// Sorts all [`RenderPhases`](RenderPhase) by painter and mask order.
pub(crate) fn sort_phase_system(MapContext { world, .. }: &mut MapContext) -> SystemResult {
    if let Some(layers) = world.resources.get_mut::<RenderPhase<LayerItem>>() {
        layers.sort();
    }
    if let Some(translucent) = world.resources.get_mut::<RenderPhase<TranslucentItem>>() {
        translucent.sort();
    }
    // Coarser masks first, so the masks of finer tiles lie over them.
    if let Some(masks) = world.resources.get_mut::<RenderPhase<TileMaskItem>>() {
        masks.sort();
    }

    Ok(())
}

/// Records the frame's render passes, submits them and presents the frame.
#[derive(Default)]
pub(crate) struct GraphRunnerSystem;

impl System for GraphRunnerSystem {
    fn name(&self) -> Cow<'static, str> {
        "graph_runner".into()
    }

    fn run(
        &mut self,
        MapContext {
            world,
            renderer:
                Renderer {
                    device,
                    queue,
                    resources: state,
                    render_graph,
                    settings,
                    ..
                },
            ..
        }: &mut MapContext,
    ) -> SystemResult {
        crate::render::gpu_timer::update_gpu_timer(world, device, queue, settings);
        render_graph.prepare().map_err(RenderError::Graph)?;
        run_graph(render_graph, device, queue, state, world).map_err(RenderError::Graph)?;

        {
            #[cfg(feature = "trace")]
            let _span = tracing::info_span!("present_frames").entered();

            let Initialized(render_target) = state.render_target.take() else {
                return Err(SystemError::Dependencies);
            };

            if let Some(surface_texture) = render_target.take_surface_texture() {
                queue.present(surface_texture);
            }

            #[cfg(feature = "tracing-tracy")]
            tracing::event!(
                tracing::Level::INFO,
                message = "finished frame",
                tracy.frame_mark = true
            );

            Ok(())
        }
    }
}

pub(crate) fn cleanup_system(
    MapContext {
        world, view_state, ..
    }: &mut MapContext,
) -> SystemResult {
    let Some((layer_item_phase, tile_mask_phase, translucent_phase)) =
        world.resources.query_mut::<(
            &mut RenderPhase<LayerItem>,
            &mut RenderPhase<TileMaskItem>,
            &mut RenderPhase<TranslucentItem>,
        )>()
    else {
        return Err(SystemError::Dependencies);
    };

    layer_item_phase.clear();
    tile_mask_phase.clear();
    translucent_phase.clear();

    view_state.update_references();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::tile_sets_match;
    use crate::coords::{WorldTileCoords, ZoomLevel};

    fn tile(x: i32, y: i32, z: u8) -> WorldTileCoords {
        WorldTileCoords {
            x,
            y,
            z: ZoomLevel::new(z),
        }
    }

    #[test]
    fn globe_tiles_match_ignores_covering_order() {
        let a = tile(3, 3, 3);
        let b = tile(4, 3, 3);
        let c = tile(3, 4, 3);
        assert!(tile_sets_match(&[b, c, a], &[a, b, c]));
        assert!(!tile_sets_match(&[a, b], &[a, b, c]));
        assert!(!tile_sets_match(&[tile(5, 3, 3), b, c], &[a, b, c]));
        assert!(tile_sets_match(&[], &[WorldTileCoords::default()]));
    }
}
