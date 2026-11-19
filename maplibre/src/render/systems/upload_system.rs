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
    last_globe: Option<(u64, bool, Vec<crate::coords::WorldTileCoords>)>,
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
    let background_tiles = crate::background::queue_system::globe_tiles(world);
    let mut memo = world
        .resources
        .get::<GlobeMeshPrepareMemo>()
        .cloned()
        .unwrap_or_default();
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

    let Some((Initialized(tile_view_pattern), Initialized(projection_resources), tile_mesh_cache)) =
        world.resources.query_mut::<(
            &mut Eventually<WgpuTileViewPattern>,
            &mut Eventually<ProjectionGpuResources>,
            &mut GlobeTileMeshCache,
        )>()
    else {
        return Err(SystemError::Dependencies);
    };

    tile_view_pattern.upload_pattern(
        queue,
        &view_proj,
        view_state.width() as f32,
        view_state.height() as f32,
        view_state.style_zoom(),
    );
    projection_resources.upload(queue, projection_data);
    tile_mesh_cache.set_globe_active(projection_data.transition > 0.0);
    if tile_mesh_cache.is_empty() {
        memo = GlobeMeshPrepareMemo::default();
    }

    let needs_raster_mesh = style.layers.iter().any(|layer| {
        !layer.is_hidden()
            && matches!(
                layer.type_.as_str(),
                "raster" | "hillshade" | "color-relief"
            )
    });
    let usages: &[(TileMeshUsage, bool)] = if needs_raster_mesh {
        &[
            (TileMeshUsage::Stencil, true),
            (TileMeshUsage::Stencil, false),
            (TileMeshUsage::Raster, true),
            (TileMeshUsage::Raster, false),
        ]
    } else {
        &[
            (TileMeshUsage::Stencil, true),
            (TileMeshUsage::Stencil, false),
        ]
    };

    if projection_data.transition > 0.0 {
        let topology_gen = tile_view_pattern.topology_generation();
        let globe_unchanged = memo
            .last_globe
            .as_ref()
            .is_some_and(|(topo_gen, raster, bg)| {
                *topo_gen == topology_gen
                    && (*raster || !needs_raster_mesh)
                    && *bg == background_tiles
            });
        if !globe_unchanged {
            for &coords in &background_tiles {
                tile_mesh_cache
                    .prepare(device, queue, coords, TileMeshUsage::Raster, false)
                    .map_err(|error| {
                        tracing::error!(%error, %coords, "unable to prepare globe background mesh");
                        SystemError::Setup
                    })?;
            }
            let mut visible_tiles = Vec::new();
            for view_tile in tile_view_pattern.iter() {
                view_tile.render(|shape| visible_tiles.push(shape.coords()));
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
            let with_raster = memo
                .last_globe
                .as_ref()
                .is_some_and(|(topo_gen, raster, _)| *topo_gen == topology_gen && *raster)
                || needs_raster_mesh;
            memo.last_globe = Some((topology_gen, with_raster, background_tiles));
            world.resources.insert(memo);
        }
    } else if !(memo.flat_prepared && (!needs_raster_mesh || memo.flat_with_raster)) {
        let coords = crate::coords::WorldTileCoords::default();
        for &(usage, generate_borders) in usages {
            tile_mesh_cache
                .prepare(device, queue, coords, usage, generate_borders)
                .map_err(|error| {
                    tracing::error!(%error, %coords, "unable to prepare globe tile mesh");
                    SystemError::Setup
                })?;
        }
        memo.flat_prepared = true;
        memo.flat_with_raster |= needs_raster_mesh;
        memo.last_globe = None;
        world.resources.insert(memo);
    }

    Ok(())
}
