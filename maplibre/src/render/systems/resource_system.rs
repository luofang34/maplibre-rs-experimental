//! Prepares GPU-owned resources by initializing them if they are uninitialized or out-of-date.

use std::{borrow::Cow, mem};

use crate::{
    context::MapContext,
    render::{
        depth_copy::DepthCopyPipeline,
        eventually::Eventually,
        projection::ProjectionGpuResources,
        resource::{BackingBufferDescriptor, RenderPipeline, Texture, TilePipeline},
        settings::{Msaa, RendererSettings},
        shaders,
        shaders::{Shader, ShaderTileMetadata},
        tile_view_pattern::{TileViewPattern, WgpuTileViewPattern, DEFAULT_TILE_VIEW_PATTERN_SIZE},
        MaskPipeline, Renderer,
    },
    tcs::system::{System, SystemError, SystemResult},
};

#[derive(Default)]
pub struct ResourceSystem;

impl System for ResourceSystem {
    fn name(&self) -> Cow<'static, str> {
        "resource_system".into()
    }

    fn run(
        &mut self,
        MapContext {
            renderer:
                Renderer {
                    settings,
                    device,
                    queue,
                    adapter,
                    resources: state,
                    render_graph,
                    ..
                },
            world,
            style,
            view_state,
        }: &mut MapContext,
    ) -> SystemResult {
        ensure_draw_capabilities_and_features(world, adapter, device, settings);
        let (size, depth_msaa, depth_usage, prep_key) = prepare_surface_and_key(
            world,
            style,
            view_state,
            settings,
            state,
            render_graph,
            device,
        )?;
        let cache_hit = world
            .resources
            .get::<CachedRenderPrepKey>()
            .is_some_and(|cached| *cached == prep_key);
        let Some((tile_view_pattern, mask_pipeline, projection_resources, depth_copy)) =
            world.resources.query_mut::<(
                &mut Eventually<WgpuTileViewPattern>,
                &mut Eventually<MaskPipeline>,
                &mut Eventually<ProjectionGpuResources>,
                &mut Eventually<DepthCopyPipeline>,
            )>()
        else {
            return Err(SystemError::Dependencies);
        };
        if cache_hit
            && matches!(projection_resources, Eventually::Initialized(_))
            && matches!(tile_view_pattern, Eventually::Initialized(_))
            && matches!(state.depth_texture, Eventually::Initialized(_))
            && matches!(state.multisampling_texture, Eventually::Initialized(_))
            && matches!(mask_pipeline, Eventually::Initialized(_))
        {
            return Ok(());
        }
        let proj_res =
            projection_resources.initialize(|| ProjectionGpuResources::new(device, queue));
        sync_depth_and_msaa(
            state,
            depth_copy,
            tile_view_pattern,
            device,
            settings,
            AttachmentSpec {
                size,
                msaa: depth_msaa,
                depth_usage,
            },
        );
        sync_mask_pipeline(
            mask_pipeline,
            proj_res,
            &state.surface,
            device,
            settings,
            prep_key.want_flat,
            prep_key.want_globe,
        );
        world.resources.insert(prep_key);

        Ok(())
    }
}

fn prepare_surface_and_key(
    world: &mut crate::tcs::world::World,
    style: &crate::style::Style,
    view_state: &crate::render::view_state::ViewState,
    settings: &RendererSettings,
    state: &mut crate::render::RenderResources,
    render_graph: &crate::render::graph::RenderGraph,
    device: &wgpu::Device,
) -> Result<
    (
        crate::window::PhysicalSize,
        Msaa,
        wgpu::TextureUsages,
        CachedRenderPrepKey,
    ),
    SystemError,
> {
    let needs_persistent_depth =
        compute_needs_persistent_depth(world, style, view_state, settings, state, render_graph);
    let supports_transient = world
        .resources
        .get::<CachedDepthFormatFeatures>()
        .is_some_and(|c| c.supports_transient);
    let transition = style.projection.as_ref().map_or(0.0, |spec| {
        spec.projection_type
            .globe_transition(view_state.zoom().value())
    });
    let want_globe = transition > 0.0;
    let want_flat = transition < 1.0 || style.terrain.is_some();
    let depth_usage = depth_texture_usage(supports_transient, needs_persistent_depth);
    let surface = &mut state.surface;
    surface.reconfigure(device);
    let size = surface.size();
    if matches!(state.render_target, Eventually::Uninitialized) {
        let view = surface
            .create_view(device)
            .map_err(crate::render::error::RenderError::from)?;
        state.render_target = Eventually::Initialized(view);
    }
    let depth_msaa = if surface.is_multisampling_supported(settings.msaa) {
        settings.msaa
    } else {
        Msaa { samples: 1 }
    };
    let prep_key = CachedRenderPrepKey {
        width: size.width(),
        height: size.height(),
        depth_msaa: depth_msaa.samples,
        depth_usage_bits: depth_usage.bits(),
        depth_readout: settings.depth_readout,
        has_eye_depth_target: state.eye_depth_target.is_some(),
        want_flat,
        want_globe,
    };
    Ok((size, depth_msaa, depth_usage, prep_key))
}

fn ensure_draw_capabilities_and_features(
    world: &mut crate::tcs::world::World,
    adapter: &wgpu::Adapter,
    device: &wgpu::Device,
    settings: &RendererSettings,
) {
    if world
        .resources
        .get::<crate::render::tracked_pass::DrawCapabilities>()
        .is_none()
    {
        world.resources.insert(
            crate::render::tracked_pass::DrawCapabilities::from_adapter_and_device(adapter, device),
        );
    }
    let cached_hit = world
        .resources
        .get::<CachedDepthFormatFeatures>()
        .is_some_and(|c| c.format == settings.depth_texture_format);
    if !cached_hit {
        let supports_transient = adapter
            .get_texture_format_features(settings.depth_texture_format)
            .allowed_usages
            .contains(wgpu::TextureUsages::TRANSIENT_ATTACHMENT);
        world.resources.insert(CachedDepthFormatFeatures {
            format: settings.depth_texture_format,
            supports_transient,
        });
    }
}

fn compute_needs_persistent_depth(
    world: &mut crate::tcs::world::World,
    style: &crate::style::Style,
    view_state: &crate::render::view_state::ViewState,
    settings: &RendererSettings,
    state: &crate::render::RenderResources,
    render_graph: &crate::render::graph::RenderGraph,
) -> bool {
    let supports_transient = world
        .resources
        .get::<CachedDepthFormatFeatures>()
        .is_some_and(|c| c.supports_transient);
    if !supports_transient {
        return true;
    }
    let world_needs = world
        .resources
        .get::<crate::render::ground::FrameGround>()
        .is_some_and(|g| g.0.is_some())
        || has_celestial_sun(world)
        || matches!(
            world
                .resources
                .get::<Eventually<crate::sdf::depth::SymbolDepth>>(),
            Some(Eventually::Initialized(sd)) if !sd.skip_depth_copy
        );
    if settings.depth_readout
        || state.eye_depth_target.is_some()
        || style.terrain.is_some()
        || world_needs
        || render_graph.has_custom_scene_depth_writers()
        || render_graph.has_named_depth_reader_passes()
    {
        return true;
    }
    let pitch_tilted = view_state.camera().get_pitch().0.abs() >= 1e-4;
    let style_rev = style.revision();
    if let Some(cached) = world.resources.get::<CachedDepthLayerScan>()
        && cached.style_rev == style_rev
        && cached.pitch_tilted == pitch_tilted
    {
        return cached.needs_persistent;
    }
    let scan = matches!(
        style.projection.as_ref().map(|p| &p.projection_type),
        Some(crate::projection::ProjectionType::VerticalPerspective)
    ) || style.layers.iter().any(|l| {
        l.type_ == "custom"
            || (pitch_tilted
                && matches!(
                    l.type_.as_str(),
                    "fill-extrusion" | "raster" | "hillshade" | "color-relief"
                ))
    });
    world.resources.insert(CachedDepthLayerScan {
        style_rev,
        pitch_tilted,
        needs_persistent: scan,
    });
    scan
}

fn depth_texture_usage(supports_transient: bool, needs_persistent: bool) -> wgpu::TextureUsages {
    if supports_transient && !needs_persistent {
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TRANSIENT_ATTACHMENT
    } else {
        // A host compositor or downstream pass reads depth back via copy or sampling.
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING
    }
}

struct AttachmentSpec {
    size: crate::window::PhysicalSize,
    msaa: Msaa,
    depth_usage: wgpu::TextureUsages,
}

fn sync_depth_texture(
    state: &mut crate::render::RenderResources,
    depth_copy: &mut Eventually<DepthCopyPipeline>,
    device: &wgpu::Device,
    settings: &RendererSettings,
    spec: &AttachmentSpec,
) {
    let (w, h) = (spec.size.width(), spec.size.height());
    let mut depth_reset = matches!(
        &state.depth_texture,
        Eventually::Initialized(tex) if tex.texture.usage() != spec.depth_usage
    );
    if depth_reset {
        state.depth_texture.take();
    }
    state.depth_texture.reinitialize(
        || {
            depth_reset = true;
            Texture::new(
                Some("depth texture"),
                device,
                settings.depth_texture_format,
                w,
                h,
                spec.msaa,
                spec.depth_usage,
            )
        },
        &(w, h),
    );
    if depth_reset {
        if let Eventually::Initialized(pipeline) = depth_copy {
            pipeline.clear_cached_bind();
        }
    }
}

fn sync_depth_and_msaa(
    state: &mut crate::render::RenderResources,
    depth_copy: &mut Eventually<DepthCopyPipeline>,
    tile_view_pattern: &mut Eventually<WgpuTileViewPattern>,
    device: &wgpu::Device,
    settings: &RendererSettings,
    spec: AttachmentSpec,
) {
    let (w, h) = (spec.size.width(), spec.size.height());
    sync_depth_texture(state, depth_copy, device, settings, &spec);
    let surface = &state.surface;
    if settings.depth_readout {
        state.depth_readout_texture.reinitialize(
            || {
                Texture::new(
                    Some("depth readout texture"),
                    device,
                    DepthCopyPipeline::TARGET_FORMAT,
                    w,
                    h,
                    Msaa { samples: 1 },
                    wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_SRC,
                )
            },
            &(w, h),
        );
    } else {
        state.depth_readout_texture.take();
    }
    if state.eye_depth_target.is_some() || settings.depth_readout {
        depth_copy.reinitialize(
            || DepthCopyPipeline::new(device, spec.msaa.samples),
            &spec.msaa.samples,
        );
    }
    let msaa_usage = surface.multisampling_usage(spec.depth_usage);
    if matches!(&state.multisampling_texture, Eventually::Initialized(Some(t)) if t.texture.usage() != msaa_usage)
    {
        state.multisampling_texture.take();
    }
    state.multisampling_texture.reinitialize(
        || {
            (settings.msaa.is_multisampling() && surface.is_multisampling_supported(settings.msaa))
                .then(|| {
                    Texture::new(
                        Some("multisampling texture"),
                        device,
                        surface.surface_format(),
                        w,
                        h,
                        settings.msaa,
                        msaa_usage,
                    )
                })
        },
        &(w, h),
    );
    init_tile_view_pattern(tile_view_pattern, device);
}

fn init_tile_view_pattern(
    tile_view_pattern: &mut Eventually<WgpuTileViewPattern>,
    device: &wgpu::Device,
) {
    tile_view_pattern.initialize(|| {
        let tile_view_buffer_desc = wgpu::BufferDescriptor {
            label: Some("tile view buffer"),
            size: mem::size_of::<ShaderTileMetadata>() as wgpu::BufferAddress
                * DEFAULT_TILE_VIEW_PATTERN_SIZE,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        };
        TileViewPattern::new(BackingBufferDescriptor::new(
            device.create_buffer(&tile_view_buffer_desc),
            tile_view_buffer_desc.size,
        ))
        .with_buffer_destroyer(wgpu::Buffer::destroy)
    });
}

fn sync_mask_pipeline(
    mask_pipeline: &mut Eventually<MaskPipeline>,
    projection_resources: &ProjectionGpuResources,
    surface: &crate::render::resource::Surface,
    device: &wgpu::Device,
    settings: &RendererSettings,
    want_flat: bool,
    want_globe: bool,
) {
    let need_flat = want_flat
        && match mask_pipeline {
            Eventually::Uninitialized => true,
            Eventually::Initialized(mask) => mask.flat.is_none(),
        };
    let need_globe = want_globe
        && match mask_pipeline {
            Eventually::Uninitialized => true,
            Eventually::Initialized(mask) => mask.globe.is_none(),
        };
    if !(matches!(mask_pipeline, Eventually::Uninitialized) || need_flat || need_globe) {
        return;
    }
    let options = crate::render::resource::TilePipelineOptions {
        depth_stencil_enabled: true,
        update_stencil: true,
        debug_stencil: false,
        wireframe: false,
        multisampling: surface.is_multisampling_supported(settings.msaa),
        textured: false,
    };
    let mask = mask_pipeline.initialize(|| MaskPipeline {
        flat: None,
        globe: None,
    });
    if need_flat {
        let flat_shader = shaders::FlatTileMaskShader {
            format: surface.surface_format(),
        };
        mask.flat = Some(
            TilePipeline::new(
                "mask_pipeline_flat".into(),
                *settings,
                flat_shader.describe_vertex(),
                flat_shader.describe_fragment(),
                options,
            )
            .describe_render_pipeline()
            .initialize_with_prefix_layouts(device, &[projection_resources.bind_group_layout()]),
        );
    }
    if need_globe {
        let globe_shader = shaders::TileMaskShader {
            format: surface.surface_format(),
            draw_colors: false,
            debug_lines: false,
        };
        let mut descriptor = TilePipeline::new(
            "mask_pipeline".into(),
            *settings,
            globe_shader.describe_vertex(),
            globe_shader.describe_fragment(),
            options,
        )
        .describe_render_pipeline();
        descriptor.primitive.cull_mode = Some(wgpu::Face::Back);
        mask.globe =
            Some(descriptor.initialize_with_prefix_layouts(
                device,
                &[projection_resources.bind_group_layout()],
            ));
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct CachedRenderPrepKey {
    width: u32,
    height: u32,
    depth_msaa: u32,
    depth_usage_bits: u32,
    depth_readout: bool,
    has_eye_depth_target: bool,
    want_flat: bool,
    want_globe: bool,
}

struct CachedDepthFormatFeatures {
    format: wgpu::TextureFormat,
    supports_transient: bool,
}

struct CachedDepthLayerScan {
    style_rev: u64,
    pitch_tilted: bool,
    needs_persistent: bool,
}

fn has_celestial_sun(world: &crate::tcs::world::World) -> bool {
    #[cfg(feature = "experimental")]
    {
        crate::celestial::lighting(world).sun.is_some()
    }
    #[cfg(not(feature = "experimental"))]
    {
        let _ = world;
        false
    }
}
