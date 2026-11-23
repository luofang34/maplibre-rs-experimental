//! Fill, line and circle commands that skip draws until GPU geometry and paint are ready.
use crate::{
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        render_phase::{LayerItem, PhaseItem, RenderCommand, RenderCommandResult},
        tile_view_pattern::WgpuTileViewPattern,
    },
    tcs::world::World,
    vector::{CirclePipeline, VectorBufferPool, VectorPipeline},
};

mod extrusion;
mod line;
mod pattern;
pub use extrusion::*;
pub use line::*;
pub use pattern::*;

#[inline]
pub(super) fn resolve_vector_entry<'w>(
    world: &'w World,
    item: &LayerItem,
) -> Option<&'w crate::vector::resource::IndexEntry> {
    let Initialized(pool) = world.resources.get::<Eventually<VectorBufferPool>>()? else {
        return None;
    };
    pool.index()
        .resolve_entry(item.tile.coords, item.style_layer, item.entry_slot)
}

pub(super) fn is_uniform_entry(world: &World, item: &LayerItem) -> bool {
    resolve_vector_entry(world, item)
        .is_some_and(crate::vector::resource::IndexEntry::is_uniform_feature_metadata)
}

/// Binds the polygon-fill pipeline and the item's view or flat projection.
pub struct SetVectorTilePipeline;
impl RenderCommand<LayerItem> for SetVectorTilePipeline {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some((Initialized(pipeline), Initialized(projection_resources))) =
            world.resources.query::<(
                &Eventually<VectorPipeline>,
                &Eventually<ProjectionGpuResources>,
            )>()
        else {
            return RenderCommandResult::Failure;
        };

        let entry = resolve_vector_entry(world, item);
        let globe = projection_resources.is_globe_active(item.projection_binding());
        let opaque = entry.is_some_and(crate::vector::resource::IndexEntry::is_opaque_color);
        let uniform =
            entry.is_some_and(crate::vector::resource::IndexEntry::is_uniform_feature_metadata);
        let Some(selected) = pipeline.select(globe, opaque, uniform) else {
            return RenderCommandResult::Failure;
        };

        pass.set_pipeline(selected);
        pass.set_bind_group(
            0,
            projection_resources.bind_group_for(item.projection_binding()),
            &[],
        );
        RenderCommandResult::Success
    }
}

/// Draws the tile's matching style-layer geometry with tile, layer and feature metadata.
///
/// With `RUNS`, a layer whose pattern varies by feature is drawn one run of features at a
/// time, each with the image its pattern names bound at the second bind group.
pub struct DrawVectorTile<const RUNS: bool = false>;
impl<const RUNS: bool> DrawVectorTile<RUNS> {
    pub(super) fn draw_prepared<'w>(
        world: &'w World,
        buffer_pool: &'w VectorBufferPool,
        tile_view_pattern: &'w WgpuTileViewPattern,
        entry: &'w crate::vector::resource::IndexEntry,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        tracing::trace!(
            "Drawing layer {:?} at {}",
            entry.style_layer.source_layer,
            entry.coords
        );
        if entry.indices_buffer_range().is_empty() {
            tracing::error!("Tried to draw a vector tile without any vertices");
            return RenderCommandResult::Failure;
        }
        let whole = pass.base_vertex(world);
        let Some((first, base_vertex, instances)) =
            bind_vector_tile_buffers(buffer_pool, tile_view_pattern, whole, item, entry, pass)
        else {
            return RenderCommandResult::Failure;
        };
        draw_vector_tile_runs::<RUNS>(
            world,
            buffer_pool,
            whole,
            item,
            entry,
            pass,
            (first, base_vertex, instances),
        );
        RenderCommandResult::Success
    }
}

impl<const RUNS: bool> RenderCommand<LayerItem> for DrawVectorTile<RUNS> {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some((Initialized(buffer_pool), Initialized(tile_view_pattern))) =
            world.resources.query::<(
                &Eventually<VectorBufferPool>,
                &Eventually<WgpuTileViewPattern>,
            )>()
        else {
            return RenderCommandResult::Failure;
        };

        let Some(entry) =
            buffer_pool
                .index()
                .resolve_entry(item.tile.coords, item.style_layer, item.entry_slot)
        else {
            return RenderCommandResult::Failure;
        };
        Self::draw_prepared(world, buffer_pool, tile_view_pattern, entry, item, pass)
    }
}

fn bind_vector_tile_buffers<'w>(
    buffer_pool: &'w VectorBufferPool,
    tile_view_pattern: &'w WgpuTileViewPattern,
    whole: bool,
    item: &LayerItem,
    entry: &'w crate::vector::resource::IndexEntry,
    pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
) -> Option<(u32, i32, std::ops::Range<u32>)> {
    let source_shape = &item.source_shape;
    pass.set_stencil_reference(u32::from(source_shape.stencil_reference()));
    let (first, base_vertex) = buffer_pool.bind_layer(pass, entry, (3, whole));
    let instances = if whole {
        let tile_instance = source_shape.instance_index()?;
        pass.set_vertex_buffer(1, tile_view_pattern.buffer().slice(..));
        tile_instance..tile_instance + 1
    } else {
        let tile_view_pattern_buffer = source_shape.buffer_range()?;
        pass.set_vertex_buffer(
            1,
            tile_view_pattern.buffer().slice(tile_view_pattern_buffer),
        );
        0..1
    };
    pass.set_vertex_buffer(
        2,
        buffer_pool
            .metadata_for(entry)
            .slice(entry.layer_metadata_buffer_range()),
    );
    Some((first, base_vertex, instances))
}

fn draw_vector_tile_runs<'w, const RUNS: bool>(
    world: &'w World,
    buffer_pool: &VectorBufferPool,
    whole: bool,
    item: &LayerItem,
    entry: &crate::vector::resource::IndexEntry,
    pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    (first, base_vertex, instances): (u32, i32, std::ops::Range<u32>),
) {
    if let Some(run) = &item.run {
        pass.draw_indexed(first + run.start..first + run.end, base_vertex, instances);
        return;
    }
    let runs = RUNS
        .then(|| buffer_pool.pattern_runs_by_id(item.tile.coords, item.style_layer))
        .flatten();
    let Some(runs) = runs else {
        if whole {
            if let Some(chunks) = entry.u16_chunks() {
                for (v_off, range) in chunks {
                    pass.draw_indexed(
                        first + range.start..first + range.end,
                        base_vertex + (*v_off as i32),
                        instances.clone(),
                    );
                }
                return;
            }
        }
        pass.draw_indexed(
            first..first + entry.indices_range().len() as u32,
            base_vertex,
            instances,
        );
        return;
    };
    let patterns = world.resources.get::<super::pattern::PatternResources>();
    let dashes = world.resources.get::<super::line_dash::LineDashResources>();
    let is_line = entry.style_layer.type_ == "line";
    for (key, range) in runs {
        let image = if is_line {
            dashes.and_then(|dashes| dashes.image_binding(*key))
        } else {
            patterns.and_then(|patterns| patterns.image_binding(*key))
        };
        if let Some(image) = image {
            pass.set_bind_group(1, image, &[]);
            pass.draw_indexed(
                first + range.start..first + range.end,
                base_vertex,
                instances.clone(),
            );
        }
    }
}

/// Where a line lies this frame, which decides its pipeline and how it is drawn.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum LineMode {
    /// On the map plane, or draped into the terrain textures.
    Flat,
    /// At heights of its own over the terrain.
    Structure,
    /// At `line-elevation` altitudes in 3D space without 2D ground stencil clipping.
    Elevated,
    /// On the terrain surface at its width in screen pixels.
    Terrain,
}

pub(super) fn line_mode_for_entry(
    entry: Option<&crate::vector::resource::IndexEntry>,
    terrain_active: bool,
) -> LineMode {
    let kind = entry.and_then(|entry| super::structures::kind(&entry.style_layer));
    if matches!(kind, Some(super::structures::StructureKind::ElevatedLine)) {
        return LineMode::Elevated;
    }
    if !terrain_active {
        return LineMode::Flat;
    }
    match kind {
        None => LineMode::Flat,
        Some(super::structures::StructureKind::Overlay) => LineMode::Terrain,
        Some(_) => LineMode::Structure,
    }
}

/// Binds the circle pipeline and the item's view or flat projection.
pub struct SetCircleTilePipeline;
impl RenderCommand<LayerItem> for SetCircleTilePipeline {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some((Initialized(pipeline), Initialized(projection_resources))) =
            world.resources.query::<(
                &Eventually<CirclePipeline>,
                &Eventually<ProjectionGpuResources>,
            )>()
        else {
            return RenderCommandResult::Failure;
        };

        let Some(selected) = pipeline.select_entry(resolve_vector_entry(world, item)) else {
            return RenderCommandResult::Failure;
        };
        pass.set_pipeline(selected);
        pass.set_bind_group(
            0,
            projection_resources.bind_group_for(item.projection_binding()),
            &[],
        );
        RenderCommandResult::Success
    }
}

/// Fused command binding the polygon-fill pipeline and drawing the bucket in one query/lookup.
pub struct DrawVectorTiles;
impl RenderCommand<LayerItem> for DrawVectorTiles {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some((
            Initialized(pipeline),
            Initialized(projection_resources),
            Initialized(buffer_pool),
            Initialized(tile_view_pattern),
        )) = world.resources.query::<(
            &Eventually<VectorPipeline>,
            &Eventually<ProjectionGpuResources>,
            &Eventually<VectorBufferPool>,
            &Eventually<WgpuTileViewPattern>,
        )>()
        else {
            return RenderCommandResult::Failure;
        };
        let Some(entry) =
            buffer_pool
                .index()
                .resolve_entry(item.tile.coords, item.style_layer, item.entry_slot)
        else {
            return RenderCommandResult::Failure;
        };
        if entry.indices_buffer_range().is_empty() {
            tracing::error!("Tried to draw a vector tile without any vertices");
            return RenderCommandResult::Failure;
        }
        let globe = projection_resources.is_globe_active(item.projection_binding());
        let Some(selected) = pipeline.select(
            globe,
            entry.is_opaque_color(),
            entry.is_uniform_feature_metadata(),
        ) else {
            return RenderCommandResult::Failure;
        };
        pass.set_pipeline(selected);
        pass.set_bind_group(
            0,
            projection_resources.bind_group_for(item.projection_binding()),
            &[],
        );
        DrawVectorTile::<false>::draw_prepared(
            world,
            buffer_pool,
            tile_view_pattern,
            entry,
            item,
            pass,
        )
    }
}

/// Fused command binding the circle pipeline and drawing the bucket in one query/lookup.
pub struct DrawCircleTiles;
impl RenderCommand<LayerItem> for DrawCircleTiles {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some((
            Initialized(pipeline),
            Initialized(projection_resources),
            Initialized(buffer_pool),
            Initialized(tile_view_pattern),
        )) = world.resources.query::<(
            &Eventually<CirclePipeline>,
            &Eventually<ProjectionGpuResources>,
            &Eventually<VectorBufferPool>,
            &Eventually<WgpuTileViewPattern>,
        )>()
        else {
            return RenderCommandResult::Failure;
        };
        let Some(entry) =
            buffer_pool
                .index()
                .resolve_entry(item.tile.coords, item.style_layer, item.entry_slot)
        else {
            return RenderCommandResult::Failure;
        };
        if entry.indices_buffer_range().is_empty() {
            tracing::error!("Tried to draw a vector tile without any vertices");
            return RenderCommandResult::Failure;
        }
        let Some(selected) = pipeline.select_entry(Some(entry)) else {
            return RenderCommandResult::Failure;
        };
        pass.set_pipeline(selected);
        pass.set_bind_group(
            0,
            projection_resources.bind_group_for(item.projection_binding()),
            &[],
        );
        DrawVectorTile::<false>::draw_prepared(
            world,
            buffer_pool,
            tile_view_pattern,
            entry,
            item,
            pass,
        )
    }
}
