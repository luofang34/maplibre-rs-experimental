//! Uploads data to the GPU which is needed for rendering.
use crate::{
    context::MapContext,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        resolved_view::ResolvedFrameView,
        tile_mesh::{GlobeTileMeshCache, TileMeshUsage},
        tile_view_pattern::WgpuTileViewPattern,
        Renderer,
    },
    tcs::system::{SystemError, SystemResult},
};

#[derive(Clone, Default)]
struct GlobeMeshPrepareMemo {
    flat_prepared: bool,
    flat_with_raster: bool,
    needs_raster_mesh: Option<(u64, bool)>,
    last_globe: Option<(u64, bool, bool, Vec<crate::coords::WorldTileCoords>)>,
}

fn globe_tiles_match(
    world: &crate::tcs::world::World,
    cached_bg: &[crate::coords::WorldTileCoords],
) -> bool {
    let drawn = crate::render::eye_covering::drawn_tiles(world).unwrap_or_default();
    let mut wrapped = drawn
        .iter()
        .copied()
        .filter(|coords| coords.wrapped().is_some())
        .peekable();
    if wrapped.peek().is_none() {
        cached_bg == [crate::coords::WorldTileCoords::default()]
    } else {
        wrapped.eq(cached_bg.iter().copied())
    }
}

pub fn upload_system(
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
            world.resources.insert(resolved);
            snapshot
        }
    };
    let style_rev = style.revision();
    let needs_raster_mesh = if let Some((_, needs)) = world
        .resources
        .get::<GlobeMeshPrepareMemo>()
        .and_then(|m| m.needs_raster_mesh)
        .filter(|(rev, _)| *rev == style_rev)
    {
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
    let uses_globe = projection_data.transition > 0.0;
    world.resources.get_or_init_mut::<GlobeMeshPrepareMemo>();

    let (topology_gen, needs_stencil_borders, meshes_empty) = {
        let Some((
            Initialized(tile_view_pattern),
            Initialized(projection_resources),
            tile_mesh_cache,
        )) = world.resources.query_mut::<(
            &mut Eventually<WgpuTileViewPattern>,
            &mut Eventually<ProjectionGpuResources>,
            &mut GlobeTileMeshCache,
        )>()
        else {
            return Err(SystemError::Dependencies);
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
        (
            tile_view_pattern.topology_generation(),
            borders,
            tile_mesh_cache.is_empty(),
        )
    };

    {
        let memo = world.resources.get_or_init_mut::<GlobeMeshPrepareMemo>();
        if meshes_empty {
            *memo = GlobeMeshPrepareMemo::default();
        }
        memo.needs_raster_mesh = Some((style_rev, needs_raster_mesh));
        if !uses_globe {
            memo.last_globe = None;
            if !(memo.flat_prepared && (!needs_raster_mesh || memo.flat_with_raster)) {
                let Some(tile_mesh_cache) = world.resources.get_mut::<GlobeTileMeshCache>() else {
                    return Err(SystemError::Dependencies);
                };
                let coords = crate::coords::WorldTileCoords::default();
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
                let memo = world.resources.get_or_init_mut::<GlobeMeshPrepareMemo>();
                memo.flat_prepared = true;
                memo.flat_with_raster |= needs_raster_mesh;
            }
            return Ok(());
        }
    }

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

    let background_tiles = crate::background::queue_system::globe_tiles(world);
    let Some((tile_mesh_cache, memo)) = world
        .resources
        .query_mut::<(&mut GlobeTileMeshCache, &mut GlobeMeshPrepareMemo)>()
    else {
        return Err(SystemError::Dependencies);
    };
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
