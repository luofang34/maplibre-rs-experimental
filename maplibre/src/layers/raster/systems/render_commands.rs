//! Raster draw commands that stop when uploaded textures, meshes or metadata are missing.

use crate::{
    raster::{cross_fade::RasterCrossFade, resource::RasterResources, RasterSourceId},
    render::{
        pass_refs::{initialized, DomainPassSlot, PassFrameRefs},
        projection::ProjectionGpuResources,
        render_phase::{
            LayerDrawKind, LayerItem, PhaseItem, ProjectionBinding, RenderCommand,
            RenderCommandResult,
        },
        tile_mesh::{GlobeTileMeshCache, TileMeshUsage},
        tile_view_pattern::WgpuTileViewPattern,
        tracked_pass::TrackedRenderPass,
    },
    tcs::world::World,
};

#[inline]
fn resolve_raster_resources<'w>(
    refs: Option<&PassFrameRefs<'w>>,
    world: &'w World,
) -> Option<&'w RasterResources> {
    PassFrameRefs::resolve_slot(refs, DomainPassSlot::RasterResources, || initialized(world))
}

#[inline]
fn resolve_raster_cross_fade<'w>(
    refs: Option<&PassFrameRefs<'w>>,
    world: &'w World,
) -> Option<&'w RasterCrossFade> {
    PassFrameRefs::resolve_slot(refs, DomainPassSlot::RasterCrossFade, || {
        world.resources.get::<RasterCrossFade>()
    })
}

/// Binds the raster pipeline and the item's view or flat projection at group zero.
pub struct SetRasterTilePipeline;
impl<P: PhaseItem> RenderCommand<P> for SetRasterTilePipeline {
    fn render<'w>(
        world: &'w World,
        item: &P,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let refs = pass.as_ref().as_ref();
        let (Some(raster_resources), Some(projection_resources)) = (
            resolve_raster_resources(refs, world),
            PassFrameRefs::resolve_projection(refs, world),
        ) else {
            return RenderCommandResult::Failure;
        };

        raster_resources.bind_pipeline(
            pass,
            projection_resources.bind_group_for(item.projection_binding()),
        );
        RenderCommandResult::Success
    }
}

/// Binds the tile's uploaded texture and sampler at the pipeline's fixed group one.
pub struct SetRasterViewBindGroup;
impl RenderCommand<LayerItem> for SetRasterViewBindGroup {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(raster_resources) = resolve_raster_resources(pass.as_ref().as_ref(), world) else {
            return RenderCommandResult::Failure;
        };

        let Some(bind_group) =
            raster_resources.layer_texture(item.style_layer.as_str(), &item.tile.coords)
        else {
            return RenderCommandResult::Failure;
        };

        pass.set_bind_group(1, bind_group, &[]);
        RenderCommandResult::Success
    }
}

/// Binds the layer's paint adjustments at group two, faded for a tile that is departing.
pub struct SetRasterPaintBindGroup;
impl RenderCommand<LayerItem> for SetRasterPaintBindGroup {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let refs = pass.as_ref().as_ref();
        let Some(raster_resources) = resolve_raster_resources(refs, world) else {
            return RenderCommandResult::Failure;
        };
        let departing = resolve_raster_cross_fade(refs, world).is_some_and(|fade| {
            fade.is_departing(
                raster_resources
                    .layer_source(item.style_layer.as_str())
                    .and_then(RasterSourceId::name),
                &item.tile.coords,
            )
        });
        let Some(bind_group) = raster_resources.layer_paint(item.style_layer, departing) else {
            return RenderCommandResult::Failure;
        };
        pass.set_bind_group(2, bind_group, &[]);
        RenderCommandResult::Success
    }
}

/// Draws a cached tile mesh with the item's stencil reference and current metadata range.
pub struct DrawRasterTile;
impl RenderCommand<LayerItem> for DrawRasterTile {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let refs = pass.as_ref().as_ref();
        let (Some(tile_view_pattern), Some(tile_mesh_cache)) = (
            crate::render::pass_refs::PassFrameRefs::resolve_tile_view_pattern(refs, world),
            crate::render::pass_refs::PassFrameRefs::resolve_tile_mesh_cache(refs, world),
        ) else {
            return RenderCommandResult::Failure;
        };

        let source_shape = &item.source_shape;
        let Some(mesh) = tile_mesh_cache.get(
            source_shape.coords(),
            TileMeshUsage::Raster,
            item.generate_borders,
        ) else {
            return RenderCommandResult::Failure;
        };

        let reference = source_shape.stencil_reference();
        // Each raster draw inverts the stencil it passes, so no pixel is drawn twice; a shape
        // drawn over another tile finds that tile's reference inverted.
        let reference = u32::from(if source_shape.is_drawn_within_another() {
            !reference
        } else {
            reference
        });

        pass.set_stencil_reference(reference);

        let whole = pass.base_vertex(world);
        pass.set_vertex_buffer(0, mesh.vertex_buffer().slice(..));
        let instances = if whole {
            let Some(tile_instance) = source_shape.instance_index() else {
                return RenderCommandResult::Failure;
            };
            pass.set_vertex_buffer(1, tile_view_pattern.buffer().slice(..));
            tile_instance..tile_instance + 1
        } else {
            let Some(tile_view_pattern_buffer) = source_shape.buffer_range() else {
                return RenderCommandResult::Failure;
            };
            pass.set_vertex_buffer(
                1,
                tile_view_pattern.buffer().slice(tile_view_pattern_buffer),
            );
            0..1
        };

        pass.set_index_buffer(mesh.index_buffer().slice(..), mesh.index_format());
        pass.draw_indexed(0..mesh.index_count(), 0, instances);

        RenderCommandResult::Success
    }
}

/// Binds the raster pipeline and image, then draws using the batched raster draw path.
pub struct DrawRasterTiles;
impl RenderCommand<LayerItem> for DrawRasterTiles {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        if try_draw_raster_group_impl(world, std::slice::from_ref(item), pass, true) {
            RenderCommandResult::Success
        } else {
            RenderCommandResult::Failure
        }
    }
}

struct RasterGroupState {
    bound_proj: Option<ProjectionBinding>,
    last_paint: Option<bool>,
    bound_whole_instances: bool,
    last_stencil: Option<u32>,
    last_vtx_buf: Option<usize>,
    last_idx_fmt: Option<(usize, wgpu::IndexFormat)>,
}

impl RasterGroupState {
    fn new() -> Self {
        Self {
            bound_proj: None,
            last_paint: None,
            bound_whole_instances: false,
            last_stencil: None,
            last_vtx_buf: None,
            last_idx_fmt: None,
        }
    }

    fn reset(&mut self) {
        self.bound_proj = None;
        self.last_paint = None;
        self.bound_whole_instances = false;
        self.last_stencil = None;
        self.last_vtx_buf = None;
        self.last_idx_fmt = None;
    }
}

struct RasterGroupResources<'w> {
    raster_resources: &'w RasterResources,
    projection_resources: &'w ProjectionGpuResources,
    tile_view_pattern: &'w WgpuTileViewPattern,
    tile_mesh_cache: &'w GlobeTileMeshCache,
    active_fade: Option<&'w RasterCrossFade>,
    source_name: Option<&'w str>,
    paint_normal: Option<&'w wgpu::BindGroup>,
    paint_departing: Option<&'w wgpu::BindGroup>,
    layer_str: &'w str,
    whole: bool,
}

fn draw_raster_item<'w>(
    res: &RasterGroupResources<'w>,
    state: &mut RasterGroupState,
    item: &LayerItem,
    pass: &mut TrackedRenderPass<'w>,
) {
    let Some(texture_bg) = res
        .raster_resources
        .layer_texture(res.layer_str, &item.tile.coords)
    else {
        return;
    };
    let departing = res
        .active_fade
        .is_some_and(|fade| fade.is_departing(res.source_name, &item.tile.coords));
    let Some(paint_bg) = (if departing {
        res.paint_departing
    } else {
        res.paint_normal
    }) else {
        return;
    };
    let source_shape = &item.source_shape;
    let Some(mesh) = res.tile_mesh_cache.get(
        source_shape.coords(),
        TileMeshUsage::Raster,
        item.generate_borders,
    ) else {
        return;
    };
    let (instances, range) = if res.whole {
        let Some(tile_instance) = source_shape.instance_index() else {
            return;
        };
        (tile_instance..tile_instance + 1, None)
    } else {
        let Some(range) = source_shape.buffer_range() else {
            return;
        };
        (0..1, Some(range))
    };
    if state.bound_proj != Some(item.projection) {
        res.raster_resources.bind_pipeline(
            pass,
            res.projection_resources.bind_group_for(item.projection),
        );
        state.bound_proj = Some(item.projection);
    }
    pass.set_bind_group(1, texture_bg, &[]);
    if state.last_paint != Some(departing) {
        pass.set_bind_group(2, paint_bg, &[]);
        state.last_paint = Some(departing);
    }
    bind_raster_item_buffers(res, state, item, mesh, range, pass);
    pass.draw_indexed(0..mesh.index_count(), 0, instances);
}

fn bind_raster_item_buffers<'w>(
    res: &RasterGroupResources<'w>,
    state: &mut RasterGroupState,
    item: &LayerItem,
    mesh: &'w crate::render::tile_mesh::GpuTileMesh,
    range: Option<std::ops::Range<u64>>,
    pass: &mut TrackedRenderPass<'w>,
) {
    let raw_ref = item.source_shape.stencil_reference();
    let reference = u32::from(if item.source_shape.is_drawn_within_another() {
        !raw_ref
    } else {
        raw_ref
    });
    if state.last_stencil != Some(reference) {
        pass.set_stencil_reference(reference);
        state.last_stencil = Some(reference);
    }
    let v_addr = std::ptr::from_ref(mesh.vertex_buffer()) as usize;
    if state.last_vtx_buf != Some(v_addr) {
        pass.set_vertex_buffer(0, mesh.vertex_buffer().slice(..));
        state.last_vtx_buf = Some(v_addr);
    }
    if let Some(range) = range {
        pass.set_vertex_buffer(1, res.tile_view_pattern.buffer().slice(range));
        state.bound_whole_instances = false;
    } else if !state.bound_whole_instances {
        pass.set_vertex_buffer(1, res.tile_view_pattern.buffer().slice(..));
        state.bound_whole_instances = true;
    }
    let idx_key = (
        std::ptr::from_ref(mesh.index_buffer()) as usize,
        mesh.index_format(),
    );
    if state.last_idx_fmt != Some(idx_key) {
        pass.set_index_buffer(mesh.index_buffer().slice(..), mesh.index_format());
        state.last_idx_fmt = Some(idx_key);
    }
}

/// Batches a contiguous raster style-layer group, resolving resources and pipeline once per layer.
pub(crate) fn try_draw_raster_group<'w>(
    world: &'w World,
    group: &[LayerItem],
    pass: &mut TrackedRenderPass<'w>,
) -> bool {
    try_draw_raster_group_impl(world, group, pass, false)
}

fn try_draw_raster_group_impl<'w>(
    world: &'w World,
    group: &[LayerItem],
    pass: &mut TrackedRenderPass<'w>,
    forced: bool,
) -> bool {
    let Some(first) = group.first() else {
        return false;
    };
    let refs = pass.as_ref().as_ref();
    let (
        Some(raster_resources),
        Some(projection_resources),
        Some(tile_view_pattern),
        Some(tile_mesh_cache),
    ) = (
        resolve_raster_resources(refs, world),
        PassFrameRefs::resolve_projection(refs, world),
        PassFrameRefs::resolve_tile_view_pattern(refs, world),
        PassFrameRefs::resolve_tile_mesh_cache(refs, world),
    )
    else {
        return !forced;
    };
    let active_fade = resolve_raster_cross_fade(refs, world).filter(|fade| !fade.is_empty());
    let style_layer = first.style_layer;
    let layer_str = style_layer.as_str();
    let res = RasterGroupResources {
        raster_resources,
        projection_resources,
        tile_view_pattern,
        tile_mesh_cache,
        active_fade,
        source_name: raster_resources
            .layer_source(layer_str)
            .and_then(RasterSourceId::name),
        paint_normal: raster_resources.layer_paint(style_layer, false),
        paint_departing: active_fade.and_then(|_| raster_resources.layer_paint(style_layer, true)),
        layer_str,
        whole: pass.base_vertex(world),
    };
    let mut state = RasterGroupState::new();
    for item in group {
        if forced || (item.draw_kind == LayerDrawKind::Raster && item.style_layer == style_layer) {
            draw_raster_item(&res, &mut state, item, pass);
        } else {
            item.draw_function.draw(pass, world, item);
            state.reset();
        }
    }
    true
}
