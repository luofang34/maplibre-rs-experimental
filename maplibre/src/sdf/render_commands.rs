use crate::{
    render::{
        pass_refs::PassFrameRefs,
        projection::ProjectionGpuResources,
        render_phase::{
            ProjectionBinding, RenderCommand, RenderCommandResult, TranslucentItem,
            UNSET_ENTRY_SLOT,
        },
        tracked_pass::TrackedRenderPass,
    },
    sdf::{textures::SymbolTextures, SymbolBufferPool, SymbolModePipelines},
    tcs::world::World,
};

struct SymbolPassResources<'w> {
    mode: &'w SymbolModePipelines,
    projection: &'w ProjectionGpuResources,
    depth: &'w super::depth::SymbolDepth,
    pool: &'w SymbolBufferPool,
    covering: &'w super::covering::SymbolCovering,
    textures: &'w SymbolTextures,
}

fn resolve_symbol_resources<'w>(
    pass: &TrackedRenderPass<'w>,
    world: &'w World,
) -> Option<SymbolPassResources<'w>> {
    let refs = pass.frame_refs();
    let symbol_pipeline = PassFrameRefs::resolve_symbol_pipeline(refs, world)?;
    let projection = PassFrameRefs::resolve_projection(refs, world)?;
    let depth = PassFrameRefs::resolve_symbol_depth(refs, world)?;
    let pool = PassFrameRefs::resolve_symbol_pool(refs, world)?;
    let covering = PassFrameRefs::resolve_symbol_covering(refs, world)?;
    let textures = PassFrameRefs::resolve_symbol_textures(refs, world)?;
    let mode = symbol_pipeline.for_globe(projection.is_globe_active(ProjectionBinding::View))?;
    Some(SymbolPassResources {
        mode,
        projection,
        depth,
        pool,
        covering,
        textures,
    })
}

fn resolve_symbol_instances<'w>(
    pass: &mut TrackedRenderPass<'w>,
    covering: &'w super::covering::SymbolCovering,
    item: &TranslucentItem,
    whole: bool,
) -> Option<std::ops::Range<u32>> {
    if whole {
        let tile_instance = item.source_shape.instance_index()?;
        pass.set_vertex_buffer(1, covering.buffer.slice(..));
        Some(tile_instance..tile_instance + 1)
    } else {
        let range = item.source_shape.buffer_range()?;
        pass.set_vertex_buffer(1, covering.buffer.slice(range));
        Some(0..1)
    }
}

fn draw_symbol_halo_pass<'w>(
    res: &SymbolPassResources<'w>,
    items: &[TranslucentItem],
    pass: &mut TrackedRenderPass<'w>,
    whole: bool,
) -> bool {
    let mut any_deferred = false;
    for item in items {
        let Some(entry) =
            res.pool
                .index()
                .resolve_entry(item.tile.coords, item.style_layer, item.entry_slot)
        else {
            continue;
        };
        let Some(binding) = res
            .textures
            .binding_by_id(item.tile.coords, item.style_layer)
        else {
            continue;
        };
        if entry.indices_buffer_range().is_empty() {
            tracing::error!("Tried to draw a vector tile without any vertices");
            continue;
        }
        let pipeline = if binding.separate_halo {
            if res.mode.fill.is_none() {
                continue;
            }
            let Some(halo) = res.mode.halo.as_ref() else {
                continue;
            };
            any_deferred = true;
            halo
        } else {
            let Some(combined) = res.mode.combined.as_ref() else {
                continue;
            };
            combined
        };
        let Some(instances) = resolve_symbol_instances(pass, res.covering, item, whole) else {
            continue;
        };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(1, &binding.group, &[]);
        let (first_idx, base_vertex) = res.pool.bind_layer(pass, entry, (2, whole));
        pass.draw_indexed(
            first_idx..first_idx + entry.indices_range().len() as u32,
            base_vertex,
            instances,
        );
    }
    any_deferred
}

fn draw_symbol_fill_pass<'w>(
    res: &SymbolPassResources<'w>,
    items: &[TranslucentItem],
    pass: &mut TrackedRenderPass<'w>,
    whole: bool,
) {
    let Some(fill) = res.mode.fill.as_ref() else {
        return;
    };
    pass.set_pipeline(fill);
    for item in items {
        let Some(entry) =
            res.pool
                .index()
                .resolve_entry(item.tile.coords, item.style_layer, item.entry_slot)
        else {
            continue;
        };
        let Some(binding) = res
            .textures
            .binding_by_id(item.tile.coords, item.style_layer)
            .filter(|binding| binding.separate_halo)
        else {
            continue;
        };
        if entry.indices_buffer_range().is_empty() {
            continue;
        }
        let Some(instances) = resolve_symbol_instances(pass, res.covering, item, whole) else {
            continue;
        };
        pass.set_bind_group(1, &binding.group, &[]);
        let (first_idx, base_vertex) = res.pool.bind_layer(pass, entry, (2, whole));
        pass.draw_indexed(
            first_idx..first_idx + entry.indices_range().len() as u32,
            base_vertex,
            instances,
        );
    }
}

/// Batches contiguous same-layer symbol items so all glyph halos draw before any fill across tiles.
pub(crate) fn try_draw_symbol_group<'w>(
    world: &'w World,
    items: &[TranslucentItem],
    pass: &mut TrackedRenderPass<'w>,
) -> bool {
    let Some(first) = items.first() else {
        return false;
    };
    if first.entry_slot == UNSET_ENTRY_SLOT || items.len() <= 1 {
        return false;
    }
    let Some(res) = resolve_symbol_resources(pass, world) else {
        return false;
    };
    let whole = pass.base_vertex(world);
    pass.set_bind_group(0, res.projection.bind_group(), &[]);
    pass.set_bind_group(2, &res.depth.binding, &[]);
    if draw_symbol_halo_pass(&res, items, pass, whole) {
        draw_symbol_fill_pass(&res, items, pass, whole);
    }
    true
}

fn draw_indices<'w>(
    pass: &mut TrackedRenderPass<'w>,
    pipeline: &'w SymbolModePipelines,
    (indices, base_vertex, instances): (std::ops::Range<u32>, i32, std::ops::Range<u32>),
    separate_halo: bool,
) -> RenderCommandResult {
    if separate_halo {
        let (Some(halo), Some(fill)) = (&pipeline.halo, &pipeline.fill) else {
            return RenderCommandResult::Failure;
        };
        // All halos must precede fills so an adjacent glyph cannot cover a finished stroke.
        pass.set_pipeline(halo);
        pass.draw_indexed(indices.clone(), base_vertex, instances.clone());
        pass.set_pipeline(fill);
        pass.draw_indexed(indices, base_vertex, instances);
    } else {
        let Some(combined) = &pipeline.combined else {
            return RenderCommandResult::Failure;
        };
        pass.set_pipeline(combined);
        pass.draw_indexed(indices, base_vertex, instances);
    }
    RenderCommandResult::Success
}

/// Fused symbol render command binding projection, depth, atlas texture, and drawing indices in
/// one resource query.
pub struct DrawSymbols;

impl RenderCommand<TranslucentItem> for DrawSymbols {
    fn render<'w>(
        world: &'w World,
        item: &TranslucentItem,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(res) = resolve_symbol_resources(pass, world) else {
            return RenderCommandResult::Failure;
        };
        let Some(entry) =
            res.pool
                .index()
                .resolve_entry(item.tile.coords, item.style_layer, item.entry_slot)
        else {
            return RenderCommandResult::Failure;
        };
        let Some(binding) = res
            .textures
            .binding_by_id(item.tile.coords, item.style_layer)
        else {
            return RenderCommandResult::Failure;
        };
        if entry.indices_buffer_range().is_empty() {
            tracing::error!("Tried to draw a vector tile without any vertices");
            return RenderCommandResult::Failure;
        }
        let whole = pass.base_vertex(world);
        let Some(instances) = resolve_symbol_instances(pass, res.covering, item, whole) else {
            return RenderCommandResult::Failure;
        };
        pass.set_bind_group(0, res.projection.bind_group(), &[]);
        pass.set_bind_group(1, &binding.group, &[]);
        pass.set_bind_group(2, &res.depth.binding, &[]);
        let (first, base_vertex) = res.pool.bind_layer(pass, entry, (2, whole));
        draw_indices(
            pass,
            res.mode,
            (
                first..first + entry.indices_range().len() as u32,
                base_vertex,
                instances,
            ),
            binding.separate_halo,
        )
    }
}
