//! Prepares GPU-owned resources by initializing them if they are uninitialized or out-of-date.
mod extrusion_setup;
mod pipeline_checks;
mod prepared_state;
mod setup;

use self::{
    pipeline_checks::{
        check_pipeline_needs, collect_prepared_flags, resolve_extrusion_needs, PipelineNeedsCheck,
    },
    prepared_state::VectorPreparedState,
    setup::PipelineSetup,
};
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
    let uses_globe = style.uses_globe_at(style_zoom);
    let terrain_needed = style.terrain.is_some() || terrain_active;
    let flags = collect_prepared_flags(world, style, style_zoom, uses_globe, terrain_active);
    let extrusion_needs = resolve_extrusion_needs(
        world,
        style.revision(),
        style_zoom,
        terrain_needed,
        uses_globe,
        &flags,
    );
    let needs_any_lines =
        flags.needs_flat_lines || flags.needs_terrain_lines || flags.needs_elevated_lines;
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
    if flags.update_dashes && needs_any_lines {
        if let Some(dashes) = world
            .resources
            .get_mut::<super::line_dash::LineDashResources>()
        {
            dashes.update(device, queue, style, style_zoom);
        }
    }
    if needs_any_lines
        && world
            .resources
            .get::<super::line_dash::LineDashResources>()
            .is_none()
    {
        return Err(SystemError::Dependencies);
    }
    let checks = check_pipeline_needs(
        world,
        style,
        style_zoom,
        uses_globe,
        terrain_needed,
        &flags,
        extrusion_needs,
    );
    let need_base = checks.need_vector_pipeline || checks.need_line_pipeline;
    let dashes_layout = if need_base || checks.need_circle_pipeline {
        world
            .resources
            .get::<super::line_dash::LineDashResources>()
            .map(|dashes| dashes.layout.clone())
    } else {
        None
    };
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
    apply_pipeline_setup(
        world,
        device,
        surface,
        settings,
        supports_base_vertex,
        need_base,
        checks,
        dashes_layout.as_ref(),
    )?;
    if flags.update_patterns {
        if let Some(patterns) = world
            .resources
            .get_mut::<super::pattern::PatternResources>()
        {
            patterns.update(device, queue, style, style_zoom);
        }
    }
    Ok(())
}

fn apply_pipeline_setup(
    world: &mut crate::tcs::world::World,
    device: &wgpu::Device,
    surface: &crate::render::resource::Surface,
    settings: &crate::render::settings::RendererSettings,
    supports_base_vertex: bool,
    need_base: bool,
    checks: PipelineNeedsCheck,
    dashes_layout: Option<&wgpu::BindGroupLayout>,
) -> Result<(), SystemError> {
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

    if !(need_base
        || checks.need_circle_pipeline
        || checks.need_extrusion_pipeline
        || checks.needs_pattern
        || checks.needs_pattern_resources)
    {
        return Ok(());
    }

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
    if need_base || checks.need_circle_pipeline {
        setup.initialize(
            vector_pipeline,
            line_pipeline,
            circle_pipeline,
            dashes_layout,
            checks.setup_needs,
        );
    }
    if checks.need_extrusion_pipeline {
        setup.initialize_extrusion(extrusion_pipeline, checks.extrusion_needs);
    }
    let pattern_pipeline = checks.needs_pattern.then(|| setup.create_pattern());
    let pattern_layout = setup.pattern;
    install_pattern_resources(
        world,
        device,
        checks.needs_pattern_resources,
        pattern_layout,
        pattern_pipeline,
    );
    Ok(())
}

fn install_pattern_resources(
    world: &mut crate::tcs::world::World,
    device: &wgpu::Device,
    needs_pattern_resources: bool,
    pattern_layout: wgpu::BindGroupLayout,
    pattern_pipeline: Option<crate::render::resource::FeaturePipelinePair>,
) {
    if needs_pattern_resources {
        world
            .resources
            .insert(super::pattern::PatternResources::new(
                device,
                pattern_layout,
            ));
    }
    if let (Some(pipeline), Some(patterns)) = (
        pattern_pipeline,
        world
            .resources
            .get_mut::<super::pattern::PatternResources>(),
    ) {
        patterns.set_pipeline(pipeline);
    }
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
