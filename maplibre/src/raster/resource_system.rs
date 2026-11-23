//! Prepares GPU-owned resources by initializing them if they are uninitialized or out-of-date.
use crate::{
    context::MapContext,
    raster::resource::RasterResources,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        resource::{RenderPipeline, TilePipeline},
        settings::Msaa,
        shaders,
        shaders::Shader,
        RenderResources, Renderer,
    },
    style::source::Source,
    tcs::system::{SystemError, SystemResult},
};

#[derive(Default)]
struct RasterStyleCache {
    rev_any_key: Option<u64>,
    style_has_any_raster: bool,
    need_key: Option<(u64, u64)>,
    style_needs_raster: bool,
    last_sources_fingerprint: Option<u64>,
}

pub fn resource_system(
    MapContext {
        world,
        style,
        view_state,
        renderer:
            Renderer {
                device,
                resources: RenderResources { surface, .. },
                settings,
                ..
            },
        ..
    }: &mut MapContext,
) -> SystemResult {
    let style_rev = style.revision();
    let style_zoom = view_state.style_zoom().value();
    let uninitialized = matches!(
        world.resources.get::<Eventually<RasterResources>>(),
        Some(Eventually::Uninitialized)
    );
    if uninitialized {
        let has_raster_or_dem_tiles = !world
            .tiles
            .pyramid_of::<crate::raster::RasterLayersDataComponent>()
            .is_empty()
            || !world
                .tiles
                .pyramid_of::<crate::raster::DemLayersComponent>()
                .is_empty();
        if !has_raster_or_dem_tiles {
            let style_needs_raster = {
                let cache = world.resources.get_or_init_mut::<RasterStyleCache>();
                let has_any = if cache.rev_any_key == Some(style_rev) {
                    cache.style_has_any_raster
                } else {
                    let any = style.terrain.is_some()
                        || style.sources.values().any(|source| {
                            matches!(
                                source,
                                Source::Image(_) | Source::Video(_) | Source::Canvas(_)
                            )
                        })
                        || style.layers.iter().any(|layer| {
                            matches!(
                                layer.type_.as_str(),
                                "raster" | "hillshade" | "color-relief"
                            ) && !layer.is_hidden()
                        });
                    cache.rev_any_key = Some(style_rev);
                    cache.style_has_any_raster = any;
                    any
                };
                if !has_any {
                    false
                } else {
                    let need_key = (style_rev, style_zoom.to_bits());
                    if cache.need_key == Some(need_key) {
                        cache.style_needs_raster
                    } else {
                        let needs = style.terrain.is_some()
                            || style.sources.values().any(|source| {
                                matches!(
                                    source,
                                    Source::Image(_) | Source::Video(_) | Source::Canvas(_)
                                )
                            })
                            || style.layers.iter().any(|layer| {
                                matches!(
                                    layer.type_.as_str(),
                                    "raster" | "hillshade" | "color-relief"
                                ) && !layer.is_hidden()
                                    && layer.is_visible_at(style_zoom)
                            });
                        cache.need_key = Some(need_key);
                        cache.style_needs_raster = needs;
                        needs
                    }
                }
            };
            if !style_needs_raster
                && world
                    .resources
                    .get::<Eventually<ProjectionGpuResources>>()
                    .is_some_and(|p| matches!(p, Initialized(_)))
            {
                return Ok(());
            }
        }
    }
    let Some((raster_resources, Initialized(projection_resources))) = world.resources.query_mut::<(
        &mut Eventually<RasterResources>,
        &mut Eventually<ProjectionGpuResources>,
    )>() else {
        return Err(SystemError::Dependencies);
    };
    let was_uninitialized = matches!(raster_resources, Eventually::Uninitialized);
    raster_resources.initialize(|| {
        let shader = shaders::RasterShader {
            format: surface.surface_format(),
        };

        let mut descriptor = TilePipeline::new(
            "raster_pipeline".into(),
            *settings,
            shader.describe_vertex(),
            shader.describe_fragment(),
            crate::render::resource::TilePipelineOptions {
                depth_stencil_enabled: true,
                update_stencil: false,
                debug_stencil: false,
                wireframe: false,
                multisampling: surface.is_multisampling_supported(settings.msaa),
                textured: true,
            },
        )
        .describe_render_pipeline();
        if let Some(state) = &mut descriptor.depth_stencil {
            // Tile references occupy the low seven bits, so even root reference zero is consumed.
            state.stencil.front.pass_op = wgpu::StencilOperation::Invert;
            state.stencil.back.pass_op = wgpu::StencilOperation::Invert;
        }
        // The raster flag gives group 1 the tile texture; the layer's paint follows.
        descriptor
            .layout
            .get_or_insert_with(Vec::new)
            .push(crate::raster::paint::layout());
        let pipeline = descriptor
            .initialize_with_prefix_layouts(device, &[projection_resources.bind_group_layout()]);
        RasterResources::new(Msaa { samples: 1 }, device, pipeline)
    });
    let sources_fp = raster_sources_fingerprint(style);
    let already_synced = !was_uninitialized
        && world
            .resources
            .get::<RasterStyleCache>()
            .is_some_and(|cache| cache.last_sources_fingerprint == Some(sources_fp));
    if !already_synced {
        if let Some(Initialized(resources)) =
            world.resources.get_mut::<Eventually<RasterResources>>()
        {
            resources.update_layer_sources(style);
        }
        let cache = world.resources.get_or_init_mut::<RasterStyleCache>();
        cache.last_sources_fingerprint = Some(sources_fp);
    }
    Ok(())
}

fn raster_sources_fingerprint(style: &crate::style::Style) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = rustc_hash::FxHasher::default();
    style.revision().hash(&mut hasher);
    style.layers.len().hash(&mut hasher);
    for layer in &style.layers {
        if matches!(
            layer.type_.as_str(),
            "raster" | "hillshade" | "color-relief"
        ) {
            layer.id.hash(&mut hasher);
            layer.type_.hash(&mut hasher);
            layer.source.hash(&mut hasher);
        }
    }
    style.sources.len().hash(&mut hasher);
    let mut sources_xor = 0_u64;
    for (name, source) in &style.sources {
        let mut entry = rustc_hash::FxHasher::default();
        name.hash(&mut entry);
        match source {
            Source::Raster(r) => {
                1_u8.hash(&mut entry);
                r.tiles
                    .as_ref()
                    .is_some_and(|t| !t.is_empty())
                    .hash(&mut entry);
            }
            Source::RasterDem(d) => {
                2_u8.hash(&mut entry);
                d.tiles
                    .as_ref()
                    .is_some_and(|t| !t.is_empty())
                    .hash(&mut entry);
            }
            _ => continue,
        }
        sources_xor ^= entry.finish();
    }
    sources_xor.hash(&mut hasher);
    hasher.finish()
}
