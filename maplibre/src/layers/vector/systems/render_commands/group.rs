//! Layer-group fast path resolving vector pipelines, projection and slab bindings once per layer.

use super::{
    draw_vector_tile_runs, extrusion_pass, line_mode_for_entry, overlaps, resolve_circle_pipeline,
    resolve_extrusion_pipeline, resolve_line_dashes, resolve_line_pipeline,
    resolve_pattern_resources, resolve_terrain_active, resolve_terrain_resources,
    resolve_vector_pipeline, resolve_vector_pool, select_extrusion_pipeline, LineMode,
};
use crate::{
    render::{
        pass_refs::PassFrameRefs,
        projection::ProjectionGpuResources,
        render_phase::{LayerDrawKind, LayerItem, ProjectionBinding},
        resource::SlabBindCache,
        tile_view_pattern::WgpuTileViewPattern,
        tracked_pass::TrackedRenderPass,
    },
    tcs::world::World,
    vector::{resource::IndexEntry, LinePipeline, VectorBufferPool},
};

struct VectorGroupResources<'w> {
    world: &'w World,
    projection_resources: &'w ProjectionGpuResources,
    buffer_pool: &'w VectorBufferPool,
    tile_view_pattern: &'w WgpuTileViewPattern,
    whole: bool,
}

#[derive(Default)]
struct VectorGroupState {
    last_pipeline: Option<*const wgpu::RenderPipeline>,
    last_proj: Option<ProjectionBinding>,
    last_stencil: Option<u32>,
    slabs: SlabBindCache,
}

impl VectorGroupState {
    fn reset(&mut self) {
        *self = Self::default();
    }

    #[inline]
    fn bind_pipeline_and_proj<'w>(
        &mut self,
        pass: &mut TrackedRenderPass<'w>,
        pipeline: &'w wgpu::RenderPipeline,
        proj: ProjectionBinding,
        proj_res: &'w ProjectionGpuResources,
    ) {
        let same_pipeline = self
            .last_pipeline
            .is_some_and(|prev| std::ptr::eq(prev, pipeline));
        if !same_pipeline {
            pass.set_pipeline(pipeline);
            self.last_pipeline = Some(pipeline);
        }
        if self.last_proj != Some(proj) {
            pass.set_bind_group(0, proj_res.bind_group_for(proj), &[]);
            self.last_proj = Some(proj);
        }
    }

    fn draw_entry<'w>(
        &mut self,
        res: &VectorGroupResources<'w>,
        item: &LayerItem,
        entry: &'w IndexEntry,
        pass: &mut TrackedRenderPass<'w>,
        runs: bool,
    ) -> bool {
        let Some(tvp_range) = item.source_shape.buffer_range() else {
            return false;
        };
        let reference = u32::from(item.source_shape.stencil_reference());
        if self.last_stencil != Some(reference) {
            pass.set_stencil_reference(reference);
            self.last_stencil = Some(reference);
        }
        let (first, base_vertex) =
            res.buffer_pool
                .bind_layer_cached(pass, entry, (3, res.whole), &mut self.slabs);
        pass.set_vertex_buffer(1, res.tile_view_pattern.buffer().slice(tvp_range));
        pass.set_vertex_buffer(
            2,
            res.buffer_pool
                .metadata_for(entry)
                .slice(entry.layer_metadata_buffer_range()),
        );
        draw_vector_tile_runs(
            res.world,
            res.buffer_pool,
            res.whole,
            item,
            entry,
            pass,
            (first, base_vertex, 0..1),
            runs,
        );
        true
    }
}

#[inline]
fn valid_entry<'w>(pool: &'w VectorBufferPool, item: &LayerItem) -> Option<&'w IndexEntry> {
    let entry = pool
        .as_ref()
        .resolve_entry(item.tile.coords, item.style_layer, item.entry_slot)?;
    if entry.indices_buffer_range().is_empty() {
        tracing::error!("Tried to draw a vector tile without any vertices");
        return None;
    }
    Some(entry)
}

fn draw_fill_group<'w>(
    res: &VectorGroupResources<'w>,
    pipeline: &'w crate::vector::VectorPipeline,
    group: &[LayerItem],
    state: &mut VectorGroupState,
    pass: &mut TrackedRenderPass<'w>,
) {
    for item in group {
        let Some(entry) = valid_entry(res.buffer_pool, item) else {
            continue;
        };
        let globe = res.projection_resources.is_globe_active(item.projection);
        let Some(selected) = pipeline.select(
            globe,
            entry.is_opaque_color(),
            entry.is_uniform_feature_metadata(),
        ) else {
            continue;
        };
        state.bind_pipeline_and_proj(pass, selected, item.projection, res.projection_resources);
        let _ = state.draw_entry(res, item, entry, pass, false);
    }
}

fn draw_circle_group<'w>(
    res: &VectorGroupResources<'w>,
    pipeline: &'w crate::vector::CirclePipeline,
    group: &[LayerItem],
    state: &mut VectorGroupState,
    pass: &mut TrackedRenderPass<'w>,
) {
    for item in group {
        let Some(entry) = valid_entry(res.buffer_pool, item) else {
            continue;
        };
        let Some(selected) = pipeline.select_entry(Some(entry)) else {
            continue;
        };
        state.bind_pipeline_and_proj(pass, selected, item.projection, res.projection_resources);
        let _ = state.draw_entry(res, item, entry, pass, false);
    }
}

fn draw_pattern_group<'w>(
    res: &VectorGroupResources<'w>,
    patterns: &'w crate::vector::pattern::PatternResources,
    group: &[LayerItem],
    state: &mut VectorGroupState,
    pass: &mut TrackedRenderPass<'w>,
) {
    let Some(first) = group.first() else {
        return;
    };
    let binding = patterns.binding(first.style_layer);
    if (binding.is_none() && !patterns.is_per_feature(first.style_layer))
        || !patterns.has_pipeline()
    {
        return;
    }
    if let Some(binding) = binding {
        pass.set_bind_group(1, binding, &[]);
    }
    for item in group {
        let Some(entry) = valid_entry(res.buffer_pool, item) else {
            continue;
        };
        let Some(pipeline) = patterns.select_pipeline(entry.is_uniform_feature_metadata()) else {
            continue;
        };
        state.bind_pipeline_and_proj(pass, pipeline, item.projection, res.projection_resources);
        let _ = state.draw_entry(res, item, entry, pass, true);
    }
}

fn select_line_pipeline<'w>(
    pipeline: &'w LinePipeline,
    proj_res: &ProjectionGpuResources,
    proj: ProjectionBinding,
    mode: LineMode,
    (is_plain, uniform): (bool, bool),
) -> Option<(&'w wgpu::RenderPipeline, bool)> {
    let (chosen, needs_dash_group) = match mode {
        LineMode::Flat => {
            if is_plain {
                let plain = if proj_res.is_globe_active(proj) {
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
    chosen.map(|pair| (pair.select(uniform), needs_dash_group))
}

type LineEnv<'w> = (
    &'w LinePipeline,
    Option<&'w crate::vector::line_dash::LineDashResources>,
    bool,
    Option<&'w crate::terrain::resources::TerrainResources>,
);

fn draw_line_group<'w>(
    res: &VectorGroupResources<'w>,
    (pipeline, dashes, terrain_active, terrain_res): LineEnv<'w>,
    group: &[LayerItem],
    state: &mut VectorGroupState,
    pass: &mut TrackedRenderPass<'w>,
) {
    let Some(first) = group.first() else {
        return;
    };
    let style_layer = first.style_layer;
    let layer_plain_solid = dashes.is_some_and(|d| d.is_plain_solid(style_layer));
    let mut cached_mode: Option<LineMode> = None;
    let mut dash_bound = false;
    for item in group {
        let Some(entry) = valid_entry(res.buffer_pool, item) else {
            continue;
        };
        let mode =
            *cached_mode.get_or_insert_with(|| line_mode_for_entry(Some(entry), terrain_active));
        let is_plain = item.run.is_none() && layer_plain_solid;
        let Some((selected, needs_dash)) = select_line_pipeline(
            pipeline,
            res.projection_resources,
            item.projection,
            mode,
            (is_plain, entry.is_uniform_feature_metadata()),
        ) else {
            continue;
        };
        if needs_dash {
            let Some(dashes) = dashes else {
                continue;
            };
            if !dash_bound {
                pass.set_bind_group(1, dashes.binding(style_layer), &[]);
                dash_bound = true;
            }
        }
        if mode == LineMode::Terrain {
            let Some(terrain) = terrain_res else {
                continue;
            };
            state.bind_pipeline_and_proj(pass, selected, item.projection, res.projection_resources);
            let tile = item.source_shape.coords();
            if terrain
                .try_for_each_draw(|draw_coords, bind_group, uniform_offset| {
                    if !overlaps(draw_coords, tile) {
                        return Ok(());
                    }
                    pass.set_bind_group(2, bind_group, &[uniform_offset]);
                    if state.draw_entry(res, item, entry, pass, true) {
                        Ok(())
                    } else {
                        Err(())
                    }
                })
                .is_err()
            {
                continue;
            }
        } else {
            state.bind_pipeline_and_proj(pass, selected, item.projection, res.projection_resources);
            let _ = state.draw_entry(res, item, entry, pass, true);
        }
    }
}

fn draw_extrusion_subchunk<'w>(
    res: &VectorGroupResources<'w>,
    pipeline: &'w crate::vector::ExtrusionPipeline,
    patterns: Option<&'w crate::vector::pattern::PatternResources>,
    sub: &[LayerItem],
    forced_kind: Option<LayerDrawKind>,
    state: &mut VectorGroupState,
    pass: &mut TrackedRenderPass<'w>,
) {
    let Some(first) = sub.first() else {
        return;
    };
    let kind = forced_kind.unwrap_or(first.draw_kind);
    let pass_kind = match kind {
        LayerDrawKind::ExtrusionDepth => extrusion_pass::DEPTH,
        LayerDrawKind::ExtrusionColor => extrusion_pass::COLOR,
        LayerDrawKind::ExtrusionPatternColor => extrusion_pass::PATTERN,
        LayerDrawKind::ExtrusionTranslucentBack => extrusion_pass::TRANSLUCENT_BACK,
        LayerDrawKind::ExtrusionTranslucentFront => extrusion_pass::TRANSLUCENT_FRONT,
        _ => {
            for item in sub {
                item.draw_function.draw(pass, res.world, item);
                state.reset();
            }
            return;
        }
    };
    let is_pattern = pass_kind == extrusion_pass::PATTERN;
    if is_pattern {
        let Some(patterns) = patterns else {
            return;
        };
        match patterns.binding(first.style_layer) {
            Some(binding) => pass.set_bind_group(1, binding, &[]),
            None if patterns.is_per_feature(first.style_layer) => {}
            None => return,
        }
    }
    for item in sub {
        let Some(mode) =
            pipeline.for_globe(res.projection_resources.is_globe_active(item.projection))
        else {
            continue;
        };
        let Some(entry) = valid_entry(res.buffer_pool, item) else {
            continue;
        };
        let Some(selected) = select_extrusion_pipeline(mode, pass_kind, entry) else {
            continue;
        };
        state.bind_pipeline_and_proj(pass, selected, item.projection, res.projection_resources);
        let _ = state.draw_entry(res, item, entry, pass, is_pattern);
    }
}

fn is_extrusion_kind(kind: LayerDrawKind) -> bool {
    matches!(
        kind,
        LayerDrawKind::ExtrusionDepth
            | LayerDrawKind::ExtrusionColor
            | LayerDrawKind::ExtrusionPatternColor
            | LayerDrawKind::ExtrusionTranslucentBack
            | LayerDrawKind::ExtrusionTranslucentFront
    )
}

/// Batches a contiguous vector style-layer group, resolving frame refs and pipelines once per layer.
pub(crate) fn try_draw_vector_group<'w>(
    world: &'w World,
    group: &[LayerItem],
    pass: &mut TrackedRenderPass<'w>,
) -> bool {
    let Some(first) = group.first() else {
        return false;
    };
    if first.draw_kind == LayerDrawKind::Raster
        || (first.draw_kind == LayerDrawKind::Other
            && !group.iter().any(|item| is_extrusion_kind(item.draw_kind)))
    {
        return false;
    }
    try_draw_vector_group_inner(world, group, pass, first.draw_kind, None)
}

pub(super) fn try_draw_vector_group_with_kind<'w>(
    world: &'w World,
    group: &[LayerItem],
    pass: &mut TrackedRenderPass<'w>,
    draw_kind: LayerDrawKind,
) -> bool {
    try_draw_vector_group_inner(world, group, pass, draw_kind, Some(draw_kind))
}

fn dispatch_line_group<'w>(
    res: &VectorGroupResources<'w>,
    group: &[LayerItem],
    state: &mut VectorGroupState,
    pass: &mut TrackedRenderPass<'w>,
) -> bool {
    let (pipeline, dashes, terrain_active, terrain_res) = {
        let refs = pass.as_ref().as_ref();
        let Some(pipeline) = resolve_line_pipeline(refs, res.world) else {
            return false;
        };
        let dashes = resolve_line_dashes(refs, res.world);
        let terrain_active = resolve_terrain_active(refs, res.world);
        let terrain_res = terrain_active
            .then(|| resolve_terrain_resources(refs, res.world))
            .flatten();
        (pipeline, dashes, terrain_active, terrain_res)
    };
    draw_line_group(
        res,
        (pipeline, dashes, terrain_active, terrain_res),
        group,
        state,
        pass,
    );
    true
}

fn dispatch_extrusion_group<'w>(
    res: &VectorGroupResources<'w>,
    group: &[LayerItem],
    (draw_kind, forced_kind): (LayerDrawKind, Option<LayerDrawKind>),
    state: &mut VectorGroupState,
    pass: &mut TrackedRenderPass<'w>,
) -> bool {
    let (pipeline, patterns) = {
        let refs = pass.as_ref().as_ref();
        let Some(pipeline) = resolve_extrusion_pipeline(refs, res.world) else {
            return false;
        };
        let patterns = (draw_kind == LayerDrawKind::ExtrusionPatternColor
            || group
                .iter()
                .any(|i| i.draw_kind == LayerDrawKind::ExtrusionPatternColor))
        .then(|| resolve_pattern_resources(refs, res.world))
        .flatten();
        (pipeline, patterns)
    };
    if forced_kind.is_some() {
        draw_extrusion_subchunk(res, pipeline, patterns, group, forced_kind, state, pass);
    } else {
        for sub in group.chunk_by(|a, b| a.draw_kind == b.draw_kind) {
            draw_extrusion_subchunk(res, pipeline, patterns, sub, None, state, pass);
        }
    }
    true
}

fn try_draw_vector_group_inner<'w>(
    world: &'w World,
    group: &[LayerItem],
    pass: &mut TrackedRenderPass<'w>,
    draw_kind: LayerDrawKind,
    forced_kind: Option<LayerDrawKind>,
) -> bool {
    let (Some(projection_resources), Some(buffer_pool), Some(tile_view_pattern)) = ({
        let refs = pass.as_ref().as_ref();
        (
            PassFrameRefs::resolve_projection(refs, world),
            resolve_vector_pool(refs, world),
            PassFrameRefs::resolve_tile_view_pattern(refs, world),
        )
    }) else {
        return forced_kind.is_none();
    };
    let res = VectorGroupResources {
        world,
        projection_resources,
        buffer_pool,
        tile_view_pattern,
        whole: pass.base_vertex(world),
    };
    let mut state = VectorGroupState::default();
    let ok = match draw_kind {
        LayerDrawKind::VectorFill => {
            let p = resolve_vector_pipeline(pass.as_ref().as_ref(), world);
            p.map(|p| draw_fill_group(&res, p, group, &mut state, pass))
                .is_some()
        }
        LayerDrawKind::VectorLine => dispatch_line_group(&res, group, &mut state, pass),
        LayerDrawKind::VectorCircle => {
            let p = resolve_circle_pipeline(pass.as_ref().as_ref(), world);
            p.map(|p| draw_circle_group(&res, p, group, &mut state, pass))
                .is_some()
        }
        LayerDrawKind::VectorPattern => {
            let p = resolve_pattern_resources(pass.as_ref().as_ref(), world);
            p.map(|p| draw_pattern_group(&res, p, group, &mut state, pass))
                .is_some()
        }
        LayerDrawKind::Raster => return false,
        _ => dispatch_extrusion_group(&res, group, (draw_kind, forced_kind), &mut state, pass),
    };
    ok || forced_kind.is_none()
}
