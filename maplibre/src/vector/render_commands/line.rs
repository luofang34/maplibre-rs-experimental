//! Line pipeline selection and terrain-draped line rendering commands.

use super::{line_mode_for_entry, DrawVectorTile, LineMode};
use crate::{
    coords::WorldTileCoords,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        render_phase::{LayerItem, PhaseItem, RenderCommand, RenderCommandResult},
        tile_view_pattern::WgpuTileViewPattern,
    },
    tcs::world::World,
    vector::{LinePipeline, VectorBufferPool},
};

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

        let entry = super::resolve_vector_entry(world, item);
        let terrain = world
            .resources
            .get::<crate::terrain::TerrainFrame>()
            .is_some_and(|frame| frame.active);
        let mode = line_mode_for_entry(entry, terrain);
        let dashes = world
            .resources
            .get::<crate::vector::line_dash::LineDashResources>();
        if !bind_line_pipeline(
            pipeline,
            projection_resources,
            dashes,
            item,
            entry,
            mode,
            pass,
        ) {
            return RenderCommandResult::Failure;
        }
        RenderCommandResult::Success
    }
}

fn bind_line_pipeline<'w>(
    pipeline: &'w LinePipeline,
    projection_resources: &'w ProjectionGpuResources,
    dashes: Option<&'w crate::vector::line_dash::LineDashResources>,
    item: &LayerItem,
    entry: Option<&crate::vector::resource::IndexEntry>,
    mode: LineMode,
    pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
) -> bool {
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
        return false;
    };
    let uniform =
        entry.is_some_and(crate::vector::resource::IndexEntry::is_uniform_feature_metadata);
    pass.set_pipeline(chosen.select(uniform));
    pass.set_bind_group(
        0,
        projection_resources.bind_group_for(item.projection_binding()),
        &[],
    );
    if needs_dash_group {
        let Some(dashes) = dashes else {
            return false;
        };
        pass.set_bind_group(1, dashes.binding(item.style_layer), &[]);
    }
    true
}

/// Draws a stroked-line bucket. A line on the terrain is drawn once over each terrain tile its
/// tile overlaps, with that tile's surface bound, since its vertices take their height from it.
pub struct DrawLineTile;

impl RenderCommand<LayerItem> for DrawLineTile {
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
        let terrain = world
            .resources
            .get::<crate::terrain::TerrainFrame>()
            .is_some_and(|frame| frame.active);
        let mode = line_mode_for_entry(Some(entry), terrain);
        draw_line_prepared(
            world,
            buffer_pool,
            tile_view_pattern,
            entry,
            mode,
            item,
            pass,
        )
    }
}

/// Fused command binding the line pipeline and drawing the line bucket in one resource query and
/// entry lookup.
pub struct DrawLineTiles;

impl RenderCommand<LayerItem> for DrawLineTiles {
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
            &Eventually<LinePipeline>,
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
        let terrain = world
            .resources
            .get::<crate::terrain::TerrainFrame>()
            .is_some_and(|frame| frame.active);
        let mode = line_mode_for_entry(Some(entry), terrain);
        let dashes = world
            .resources
            .get::<crate::vector::line_dash::LineDashResources>();
        if !bind_line_pipeline(
            pipeline,
            projection_resources,
            dashes,
            item,
            Some(entry),
            mode,
            pass,
        ) {
            return RenderCommandResult::Failure;
        }
        draw_line_prepared(
            world,
            buffer_pool,
            tile_view_pattern,
            entry,
            mode,
            item,
            pass,
        )
    }
}

fn draw_line_prepared<'w>(
    world: &'w World,
    buffer_pool: &'w VectorBufferPool,
    tile_view_pattern: &'w WgpuTileViewPattern,
    entry: &'w crate::vector::resource::IndexEntry,
    mode: LineMode,
    item: &LayerItem,
    pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
) -> RenderCommandResult {
    if mode != LineMode::Terrain {
        return DrawVectorTile::<true>::draw_prepared(
            world,
            buffer_pool,
            tile_view_pattern,
            entry,
            item,
            pass,
        );
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
        if let RenderCommandResult::Failure = DrawVectorTile::<true>::draw_prepared(
            world,
            buffer_pool,
            tile_view_pattern,
            entry,
            item,
            pass,
        ) {
            return RenderCommandResult::Failure;
        }
    }
    RenderCommandResult::Success
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
