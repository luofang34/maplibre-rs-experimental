//! Prepares GPU-owned resources by initializing them if they are uninitialized or out-of-date.
mod extrusion_setup;
mod prepared_state;
mod setup;

use self::{prepared_state::VectorPreparedState, setup::PipelineSetup};
use crate::{
    context::MapContext,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        RenderResources, Renderer,
    },
    tcs::system::{SystemError, SystemResult},
    vector::{
        resource::BufferPool, CirclePipeline, ExtrusionPipeline, LinePipeline, VectorBufferPool,
        VectorPipeline,
    },
};

pub fn resource_system(
    MapContext {
        world,
        style,
        view_state,
        renderer:
            Renderer {
                adapter,
                device,
                queue,
                resources: RenderResources { surface, .. },
                settings,
                ..
            },
        ..
    }: &mut MapContext,
) -> SystemResult {
    let style_zoom = view_state.style_zoom().value();
    let terrain_active = world
        .resources
        .get::<crate::terrain::TerrainFrame>()
        .is_some_and(|frame| frame.active);
    let mut prepared = world
        .resources
        .remove::<VectorPreparedState>()
        .unwrap_or_default();
    prepared.sync_style(style, terrain_active);
    let update_dashes = prepared.should_update_dashes(style);
    let update_patterns = prepared.should_update_patterns(style);
    let needs_fill = prepared.needs_fill;
    let needs_flat_lines = prepared.needs_flat_lines;
    let needs_terrain_lines = prepared.needs_terrain_lines;
    let needs_elevated_lines = prepared.needs_elevated_lines;
    let needs_circle = prepared.needs_circle;
    let needs_circle_feature_stroke = needs_circle && prepared.needs_circle_feature_stroke;
    let needs_circle_dynamic_radius = needs_circle && prepared.needs_circle_dynamic_radius;
    let uses_globe = style.uses_globe_at(style_zoom);
    let terrain_needed = style.terrain.is_some() || terrain_active;
    let needs_extrusion = prepared.needs_extrusion_at_zoom(style_zoom, uses_globe);
    let needs_extrusion_opaque = needs_extrusion && prepared.needs_extrusion_opaque;
    let needs_extrusion_pattern = needs_extrusion && prepared.needs_extrusion_pattern;
    let needs_extrusion_translucent = needs_extrusion && prepared.needs_extrusion_translucent;
    let (has_compact_extrusion, has_standard_extrusion) = if needs_extrusion {
        detect_extrusion_encodings(world, &prepared, style_zoom, terrain_needed)
    } else {
        (false, false)
    };
    let extrusion_needs = extrusion_setup::ExtrusionPipelineNeeds {
        globe: uses_globe,
        standard_opaque: needs_extrusion_opaque && has_standard_extrusion,
        compact_opaque: needs_extrusion_opaque && has_compact_extrusion,
        pattern: needs_extrusion_opaque && needs_extrusion_pattern && has_standard_extrusion,
        standard_translucent: needs_extrusion_translucent && has_standard_extrusion,
        compact_translucent: needs_extrusion_translucent && has_compact_extrusion,
        clear_mark: needs_extrusion_opaque && (has_compact_extrusion || has_standard_extrusion),
    };
    let has_pattern_paint = prepared.has_pattern_paint;
    let needs_plain_flat_lines = needs_flat_lines && (!uses_globe || terrain_needed);
    let needs_plain_globe_lines = needs_flat_lines && uses_globe;
    let needs_full_flat_lines = needs_flat_lines && prepared.has_dash_or_pattern_line;
    let needs_flat_fill_opaque =
        needs_fill && prepared.needs_fill_opaque && (!uses_globe || terrain_needed);
    let needs_flat_fill_blended =
        needs_fill && prepared.needs_fill_blended && (!uses_globe || terrain_needed);
    let needs_globe_fill_opaque = needs_fill && prepared.needs_fill_opaque && uses_globe;
    let needs_globe_fill_blended = needs_fill && prepared.needs_fill_blended && uses_globe;
    world.resources.insert(prepared);

    let needs_any_lines = needs_flat_lines || needs_terrain_lines || needs_elevated_lines;
    if needs_any_lines
        && world
            .resources
            .get::<super::line_dash::LineDashResources>()
            .is_none()
    {
        world
            .resources
            .insert(super::line_dash::LineDashResources::new(device, queue));
    }
    if update_dashes && needs_any_lines {
        if let Some(dashes) = world
            .resources
            .get_mut::<super::line_dash::LineDashResources>()
        {
            dashes.update(device, queue, style, style_zoom);
        }
    }
    let dashes_layout = world
        .resources
        .get::<super::line_dash::LineDashResources>()
        .map(|dashes| dashes.layout.clone());
    if needs_any_lines && dashes_layout.is_none() {
        return Err(SystemError::Dependencies);
    }
    let pattern_res = world.resources.get::<super::pattern::PatternResources>();
    let missing_pattern_resources = pattern_res.is_none();
    let missing_pattern_pipeline = pattern_res.is_none_or(|patterns| patterns.pipeline().is_none());
    let supports_base_vertex = world
        .resources
        .get::<crate::render::tracked_pass::DrawCapabilities>()
        .map_or_else(
            || {
                crate::render::tracked_pass::DrawCapabilities::from_adapter_and_device(
                    adapter, device,
                )
                .base_vertex
            },
            |caps| caps.base_vertex,
        );
    let Some((
        buffer_pool,
        vector_pipeline,
        line_pipeline,
        circle_pipeline,
        extrusion_pipeline,
        Initialized(projection_resources),
    )) = world.resources.query_mut::<(
        &mut Eventually<VectorBufferPool>,
        &mut Eventually<VectorPipeline>,
        &mut Eventually<LinePipeline>,
        &mut Eventually<CirclePipeline>,
        &mut Eventually<ExtrusionPipeline>,
        &mut Eventually<ProjectionGpuResources>,
    )>()
    else {
        return Err(SystemError::Dependencies);
    };

    buffer_pool.initialize(|| {
        BufferPool::from_device_with_sizes(device, settings.buffer_pools)
            .with_u16_chunks(supports_base_vertex)
            .with_decoupled_feature_metadata()
            .with_shared_layer_metadata()
    });

    let need_vector_pipeline = needs_fill
        && !matches!(vector_pipeline, Initialized(vp) if
            (!needs_flat_fill_opaque || vp.flat_opaque.is_some())
            && (!needs_flat_fill_blended || vp.flat_blended.is_some())
            && (!needs_globe_fill_opaque || vp.globe_opaque.is_some())
            && (!needs_globe_fill_blended || vp.globe_blended.is_some()));
    let need_line_pipeline = needs_any_lines
        && !matches!(line_pipeline, Initialized(lp) if
            (!needs_plain_flat_lines || lp.flat_plain.is_some())
            && (!needs_plain_globe_lines || lp.globe_plain.is_some())
            && (!needs_full_flat_lines || lp.flat.is_some())
            && (!needs_terrain_lines || (lp.structure.is_some() && lp.terrain.is_some()))
            && (!needs_elevated_lines || lp.elevated.is_some()));
    let need_base_pipelines = need_vector_pipeline || need_line_pipeline;
    let need_circle_pipeline = needs_circle
        && !matches!(circle_pipeline, Initialized(cp) if
            (!needs_circle_feature_stroke || cp.feature_stroke.is_some())
            && (!needs_circle_dynamic_radius || cp.dynamic_radius.is_some()));
    let need_extrusion_pipeline = needs_extrusion
        && (has_compact_extrusion || has_standard_extrusion)
        && !matches!(extrusion_pipeline, Initialized(ep) if
        (!extrusion_needs.clear_mark || ep.clear_mark.is_some())
            && (!(extrusion_needs.standard_opaque
                || extrusion_needs.compact_opaque
                || extrusion_needs.pattern
                || extrusion_needs.standard_translucent
                || extrusion_needs.compact_translucent)
                || ep.for_globe(uses_globe).is_some_and(|mode| {
                    (!extrusion_needs.standard_opaque
                        || (mode.depth.is_some() && mode.color.is_some()))
                        && (!extrusion_needs.compact_opaque
                            || (mode.compact_depth.is_some() && mode.compact_color.is_some()))
                        && (!extrusion_needs.pattern || mode.pattern.is_some())
                        && (!extrusion_needs.standard_translucent
                            || (mode.translucent_back.is_some()
                                && mode.translucent_front.is_some()))
                        && (!extrusion_needs.compact_translucent
                            || (mode.compact_translucent_back.is_some()
                                && mode.compact_translucent_front.is_some()))
                })));
    let needs_pattern = missing_pattern_pipeline
        && has_pattern_paint
        && style.layers.iter().any(|layer| {
            layer.paint.as_ref().is_some_and(|paint| {
                super::pattern::pattern_name(paint, style_zoom).is_some()
                    || super::pattern::per_feature_pattern(paint).is_some()
            })
        });

    let pattern_pipeline = if need_base_pipelines
        || need_circle_pipeline
        || need_extrusion_pipeline
        || needs_pattern
        || missing_pattern_resources
    {
        let setup = PipelineSetup {
            device,
            settings: *settings,
            format: surface.surface_format(),
            multisampling: surface.is_multisampling_supported(settings.msaa),
            projection: projection_resources.bind_group_layout(),
            pattern: crate::render::resource::create_bind_group_layout_cached(
                device,
                Some("pattern layout"),
                &super::pattern::layout_entries(),
            ),
        };
        if need_base_pipelines || need_circle_pipeline {
            setup.initialize(
                vector_pipeline,
                line_pipeline,
                circle_pipeline,
                dashes_layout.as_ref(),
                setup::VectorPipelineNeeds {
                    flat_fill_opaque: needs_flat_fill_opaque,
                    flat_fill_blended: needs_flat_fill_blended,
                    globe_fill_opaque: needs_globe_fill_opaque,
                    globe_fill_blended: needs_globe_fill_blended,
                    plain_flat_lines: needs_plain_flat_lines,
                    plain_globe_lines: needs_plain_globe_lines,
                    flat_lines: needs_full_flat_lines,
                    terrain_lines: needs_terrain_lines,
                    elevated_lines: needs_elevated_lines,
                    circle: needs_circle,
                    circle_feature_stroke: needs_circle_feature_stroke,
                    circle_dynamic_radius: needs_circle_dynamic_radius,
                },
            );
        }
        if need_extrusion_pipeline {
            setup.initialize_extrusion(extrusion_pipeline, extrusion_needs);
        }
        let created_pattern = needs_pattern.then(|| setup.create_pattern());
        let pattern_layout = setup.pattern;
        if missing_pattern_resources {
            world
                .resources
                .insert(super::pattern::PatternResources::new(
                    device,
                    pattern_layout,
                ));
        }
        created_pattern
    } else {
        None
    };
    if let (Some(pipeline), Some(patterns)) = (
        pattern_pipeline,
        world
            .resources
            .get_mut::<super::pattern::PatternResources>(),
    ) {
        patterns.set_pipeline(pipeline);
    }
    if update_patterns {
        if let Some(patterns) = world
            .resources
            .get_mut::<super::pattern::PatternResources>()
        {
            patterns.update(device, queue, style, style_zoom);
        }
    }
    Ok(())
}

fn detect_extrusion_encodings(
    world: &crate::tcs::world::World,
    prepared: &VectorPreparedState,
    style_zoom: f64,
    terrain_needed: bool,
) -> (bool, bool) {
    let (mut needs_compact, mut needs_standard) = scan_pool_extrusion_encodings(world, style_zoom);
    if (needs_compact && needs_standard)
        || world
            .tiles
            .pyramid_of::<crate::vector::VectorLayerBucketComponent>()
            .is_empty()
    {
        return (needs_compact, needs_standard);
    }
    for tile in world.tiles.tiles.values() {
        if let Some(buckets) = world
            .tiles
            .query::<&crate::vector::VectorLayerBucketComponent>(tile.coords)
        {
            for layer in &buckets.layers {
                if let crate::vector::VectorLayerBucket::AvailableLayer(bucket) = layer {
                    inspect_extrusion_bucket(
                        bucket,
                        prepared,
                        style_zoom,
                        terrain_needed,
                        &mut needs_compact,
                        &mut needs_standard,
                    );
                    if needs_compact && needs_standard {
                        return (true, true);
                    }
                }
            }
        }
        if let Some(pending) = world
            .tiles
            .query::<&crate::vector::content::LayerReplacements>(tile.coords)
        {
            for bucket in &pending.vector {
                inspect_extrusion_bucket(
                    bucket,
                    prepared,
                    style_zoom,
                    terrain_needed,
                    &mut needs_compact,
                    &mut needs_standard,
                );
                if needs_compact && needs_standard {
                    return (true, true);
                }
            }
        }
    }
    (needs_compact, needs_standard)
}

fn scan_pool_extrusion_encodings(
    world: &crate::tcs::world::World,
    style_zoom: f64,
) -> (bool, bool) {
    let mut needs_compact = false;
    let mut needs_standard = false;
    if let Some(Eventually::Initialized(pool)) =
        world.resources.get::<Eventually<VectorBufferPool>>()
    {
        for entry in pool.index().iter().flatten() {
            if entry.style_layer.type_ == "fill-extrusion"
                && entry.style_layer.is_visible_at(style_zoom)
            {
                if entry.is_compact_extrusion() {
                    needs_compact = true;
                } else {
                    needs_standard = true;
                }
                if needs_compact && needs_standard {
                    return (true, true);
                }
            }
        }
    }
    (needs_compact, needs_standard)
}

fn inspect_extrusion_bucket(
    bucket: &crate::vector::AvailableVectorLayerBucket,
    prepared: &VectorPreparedState,
    style_zoom: f64,
    terrain_needed: bool,
    needs_compact: &mut bool,
    needs_standard: &mut bool,
) {
    if !bucket.buffer.has_drawable_geometry()
        || !prepared.is_visible_extrusion_layer(&bucket.style_layer_id, style_zoom)
    {
        return;
    }
    if let Some(pre) = bucket.buffer.prepacked_gpu() {
        if pre.compact_extrusion {
            *needs_compact = true;
            if terrain_needed || !bucket.feature_colors.is_empty() {
                *needs_standard = true;
            }
        } else {
            *needs_standard = true;
        }
    } else if !bucket.buffer.buffer.vertices.is_empty() {
        if !terrain_needed
            && !prepared.needs_extrusion_pattern
            && bucket.feature_colors.is_empty()
            && crate::render::shaders::can_encode_compact_extrusion(&bucket.buffer.buffer.vertices)
        {
            *needs_compact = true;
        } else {
            *needs_standard = true;
        }
    } else {
        *needs_compact = true;
        *needs_standard = true;
    }
}
