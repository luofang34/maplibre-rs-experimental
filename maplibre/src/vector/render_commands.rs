//! Fill, line and circle commands that skip draws until GPU geometry and paint are ready.
use crate::{
    coords::WorldTileCoords,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        render_phase::{LayerItem, PhaseItem, RenderCommand, RenderCommandResult},
        tile_view_pattern::WgpuTileViewPattern,
    },
    tcs::world::World,
    vector::{CirclePipeline, LinePipeline, VectorBufferPool, VectorPipeline},
};

mod extrusion;
pub use extrusion::*;

#[inline]
fn resolve_vector_entry<'w>(
    world: &'w World,
    item: &LayerItem,
) -> Option<&'w crate::vector::resource::IndexEntry> {
    let Initialized(pool) = world.resources.get::<Eventually<VectorBufferPool>>()? else {
        return None;
    };
    pool.index()
        .resolve_entry(item.tile.coords, item.style_layer, item.entry_slot)
}

fn is_uniform_entry(world: &World, item: &LayerItem) -> bool {
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

        let source_shape = &item.source_shape;

        let reference = u32::from(source_shape.stencil_reference());

        tracing::trace!(
            "Drawing layer {:?} at {}",
            entry.style_layer.source_layer,
            entry.coords
        );

        let index_range = entry.indices_buffer_range();

        if index_range.is_empty() {
            tracing::error!("Tried to draw a vector tile without any vertices");
            return RenderCommandResult::Failure;
        }

        pass.set_stencil_reference(reference);

        let whole = world
            .resources
            .get::<crate::render::tracked_pass::DrawCapabilities>()
            .is_some_and(|capabilities| capabilities.base_vertex);
        let (first, base_vertex) = buffer_pool.bind_layer(pass, entry, (3, whole));
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
        pass.set_vertex_buffer(
            2,
            buffer_pool
                .metadata_for(entry)
                .slice(entry.layer_metadata_buffer_range()),
        );
        if let Some(run) = &item.run {
            pass.draw_indexed(first + run.start..first + run.end, base_vertex, instances);
            return RenderCommandResult::Success;
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
                    return RenderCommandResult::Success;
                }
            }
            pass.draw_indexed(
                first..first + entry.indices_range().len() as u32,
                base_vertex,
                instances,
            );
            return RenderCommandResult::Success;
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
            let Some(image) = image else {
                continue;
            };
            pass.set_bind_group(1, image, &[]);
            pass.draw_indexed(
                first + range.start..first + range.end,
                base_vertex,
                instances.clone(),
            );
        }

        RenderCommandResult::Success
    }
}

/// Where a line lies this frame, which decides its pipeline and how it is drawn.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LineMode {
    /// On the map plane, or draped into the terrain textures.
    Flat,
    /// At heights of its own over the terrain.
    Structure,
    /// At `line-elevation` altitudes in 3D space without 2D ground stencil clipping.
    Elevated,
    /// On the terrain surface at its width in screen pixels.
    Terrain,
}

fn line_mode(world: &World, item: &LayerItem) -> LineMode {
    let kind = world
        .resources
        .get::<Eventually<VectorBufferPool>>()
        .and_then(|pool| match pool {
            Initialized(pool) => {
                pool.index()
                    .resolve_entry(item.tile.coords, item.style_layer, item.entry_slot)
            }
            _ => None,
        })
        .and_then(|entry| super::structures::kind(&entry.style_layer));
    if matches!(kind, Some(super::structures::StructureKind::ElevatedLine)) {
        return LineMode::Elevated;
    }
    let terrain = world
        .resources
        .get::<crate::terrain::TerrainFrame>()
        .is_some_and(|frame| frame.active);
    if !terrain {
        return LineMode::Flat;
    }
    match kind {
        None => LineMode::Flat,
        Some(super::structures::StructureKind::Overlay) => LineMode::Terrain,
        Some(_) => LineMode::Structure,
    }
}

/// Binds the line pipeline and dash atlas; elevated structures and lines on the terrain surface
/// use depth-aware drawing on terrain.
pub struct SetLineTilePipeline;
impl RenderCommand<LayerItem> for SetLineTilePipeline {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some((Initialized(pipeline), Initialized(projection_resources))) =
            world.resources.query::<(
                &Eventually<LinePipeline>,
                &Eventually<ProjectionGpuResources>,
            )>()
        else {
            return RenderCommandResult::Failure;
        };

        let mode = line_mode(world, item);
        let dashes = world.resources.get::<super::line_dash::LineDashResources>();
        let (chosen, needs_dash_group) = match mode {
            LineMode::Flat => {
                let is_plain = item.run.is_none()
                    && dashes.is_some_and(|dashes| dashes.is_plain_solid(item.style_layer));
                if is_plain {
                    let plain = if projection_resources.is_globe_active(item.projection_binding()) {
                        pipeline.globe_plain.as_ref()
                    } else {
                        pipeline.flat_plain.as_ref()
                    };
                    if let Some(plain) = plain {
                        (Some(plain), false)
                    } else {
                        (pipeline.flat.as_ref(), true)
                    }
                } else {
                    (pipeline.flat.as_ref(), true)
                }
            }
            LineMode::Structure => (pipeline.structure.as_ref(), true),
            LineMode::Elevated => (pipeline.elevated.as_ref(), true),
            LineMode::Terrain => (pipeline.terrain.as_ref(), true),
        };
        let Some(chosen) = chosen else {
            return RenderCommandResult::Failure;
        };
        pass.set_pipeline(chosen.select(is_uniform_entry(world, item)));
        pass.set_bind_group(
            0,
            projection_resources.bind_group_for(item.projection_binding()),
            &[],
        );
        if needs_dash_group {
            let Some(dashes) = dashes else {
                return RenderCommandResult::Failure;
            };
            pass.set_bind_group(1, dashes.binding(item.style_layer), &[]);
        }
        RenderCommandResult::Success
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

/// Binds the pattern pipeline and the pattern of the item's layer.
pub struct SetPatternTilePipeline;
impl RenderCommand<LayerItem> for SetPatternTilePipeline {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let (Some(patterns), Some(Initialized(projection_resources))) = (
            world.resources.get::<super::pattern::PatternResources>(),
            world.resources.get::<Eventually<ProjectionGpuResources>>(),
        ) else {
            return RenderCommandResult::Failure;
        };
        let binding = patterns.binding(item.style_layer);
        if binding.is_none() && !patterns.is_per_feature(item.style_layer) {
            return RenderCommandResult::Failure;
        }
        let Some(pipeline) = patterns.pipeline() else {
            return RenderCommandResult::Failure;
        };
        pass.set_pipeline(pipeline.select(is_uniform_entry(world, item)));
        pass.set_bind_group(
            0,
            projection_resources.bind_group_for(item.projection_binding()),
            &[],
        );
        if let Some(binding) = binding {
            pass.set_bind_group(1, binding, &[]);
        }
        RenderCommandResult::Success
    }
}

/// Binds and draws a polygon bucket filled with a repeating image.
pub type DrawPatternTiles = (SetPatternTilePipeline, DrawVectorTile<true>);
/// Binds the image of the item's layer at the second bind group.
pub struct SetPatternBindGroup;
impl RenderCommand<LayerItem> for SetPatternBindGroup {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(patterns) = world.resources.get::<super::pattern::PatternResources>() else {
            return RenderCommandResult::Failure;
        };
        match patterns.binding(item.style_layer) {
            Some(binding) => pass.set_bind_group(1, binding, &[]),
            None if patterns.is_per_feature(item.style_layer) => {}
            None => return RenderCommandResult::Failure,
        }
        RenderCommandResult::Success
    }
}

/// Binds and draws a polygon-fill bucket when both commands succeed.
pub type DrawVectorTiles = (SetVectorTilePipeline, DrawVectorTile);
/// Draws a stroked-line bucket. A line on the terrain is drawn once over each terrain tile its
/// tile overlaps, with that tile's surface bound, since its vertices take their height from it.
pub struct DrawLineTile;
impl RenderCommand<LayerItem> for DrawLineTile {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        if line_mode(world, item) != LineMode::Terrain {
            return DrawVectorTile::<true>::render(world, item, pass);
        }
        let Some(Initialized(terrain)) = world
            .resources
            .get::<Eventually<crate::terrain::resources::TerrainResources>>()
        else {
            return RenderCommandResult::Failure;
        };
        let tile = item.source_shape.coords();
        for draw in terrain
            .draws()
            .iter()
            .filter(|draw| overlaps(draw.coords, tile))
        {
            pass.set_bind_group(2, draw.bind_group.as_ref(), &[draw.uniform_offset]);
            if let RenderCommandResult::Failure = DrawVectorTile::<true>::render(world, item, pass)
            {
                return RenderCommandResult::Failure;
            }
        }
        RenderCommandResult::Success
    }
}

/// Whether one tile contains the other.
fn overlaps(a: WorldTileCoords, b: WorldTileCoords) -> bool {
    let (coarse, fine) = if u8::from(a.z) <= u8::from(b.z) {
        (a, b)
    } else {
        (b, a)
    };
    let levels = u8::from(fine.z) - u8::from(coarse.z);
    fine.x >> levels == coarse.x && fine.y >> levels == coarse.y
}

/// Binds and draws a stroked-line bucket when both commands succeed.
pub type DrawLineTiles = (SetLineTilePipeline, DrawLineTile);
/// Binds and draws a circle bucket when both commands succeed.
pub type DrawCircleTiles = (SetCircleTilePipeline, DrawVectorTile);
