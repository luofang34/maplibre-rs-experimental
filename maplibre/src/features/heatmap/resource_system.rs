//! Builds the density and composite pipelines once the renderer is up.

use crate::{
    context::MapContext,
    heatmap::resources::HeatmapResources,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        resource::{RenderPipeline, TilePipeline, TilePipelineOptions},
        shaders::{HeatmapCompositeShader, HeatmapDensityShader, Shader, DENSITY_FORMAT},
        RenderResources, Renderer,
    },
    schedule::system::{SystemError, SystemResult},
};

/// Bind group of the composite's ramp texture and opacity, after the density texture group.
fn ramp_layout() -> Vec<wgpu::BindGroupLayoutEntry> {
    vec![
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                multisampled: false,
                view_dimension: wgpu::TextureViewDimension::D2,
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
    ]
}

/// Marks that the device cannot draw density targets, so the failure is reported once.
struct DensityUnsupported;

/// Whether the density format can be rendered to, blended and sampled with filtering. Core
/// WebGPU guarantees it; a WebGL2 context needs the half-float colour buffer extension.
fn density_format_supported(adapter: &wgpu::Adapter) -> bool {
    let features = adapter.get_texture_format_features(DENSITY_FORMAT);
    features
        .allowed_usages
        .contains(wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING)
        && features.flags.contains(
            wgpu::TextureFormatFeatureFlags::BLENDABLE
                | wgpu::TextureFormatFeatureFlags::FILTERABLE,
        )
}

#[derive(Default)]
struct HeatmapResourceCache {
    rev: Option<u64>,
    has_heatmap: bool,
    density_supported: Option<bool>,
}

pub fn resource_system(
    MapContext {
        world,
        style,
        renderer:
            Renderer {
                device,
                adapter,
                resources: RenderResources { surface, .. },
                settings,
                ..
            },
        ..
    }: &mut MapContext,
) -> SystemResult {
    let style_rev = style.revision();
    let has_heatmap = match world.resources.get::<HeatmapResourceCache>() {
        Some(cache) if cache.rev == Some(style_rev) => cache.has_heatmap,
        _ => {
            let has = style.layers.iter().any(|layer| {
                layer.type_ == "heatmap"
                    || matches!(
                        layer.paint,
                        Some(crate::style::layer::LayerPaint::Heatmap(_))
                    )
            });
            let cache = world.resources.get_or_init_mut::<HeatmapResourceCache>();
            cache.rev = Some(style_rev);
            cache.has_heatmap = has;
            has
        }
    };
    if !has_heatmap
        && world
            .resources
            .get::<Eventually<HeatmapResources>>()
            .is_some_and(|res| matches!(res, Eventually::Uninitialized))
    {
        return Ok(());
    }
    if !has_heatmap {
        return Ok(());
    }
    let supported = match world
        .resources
        .get::<HeatmapResourceCache>()
        .and_then(|cache| cache.density_supported)
    {
        Some(supported) => supported,
        None => {
            let supported = density_format_supported(adapter);
            world
                .resources
                .get_or_init_mut::<HeatmapResourceCache>()
                .density_supported = Some(supported);
            supported
        }
    };
    if !supported {
        if world.resources.get::<DensityUnsupported>().is_none() {
            tracing::error!(
                format = ?DENSITY_FORMAT,
                "this device cannot render and blend half-float density targets; heatmap layers are not drawn"
            );
            world.resources.insert(DensityUnsupported);
        }
        return Ok(());
    }
    let Some((heatmap_resources, Initialized(projection_resources))) =
        world.resources.query_mut::<(
            &mut Eventually<HeatmapResources>,
            &mut Eventually<ProjectionGpuResources>,
        )>()
    else {
        return Err(SystemError::Dependencies);
    };

    heatmap_resources.initialize(|| {
        let shader = HeatmapDensityShader;
        // The density target has no depth or stencil, one sample and no tile clipping: every
        // point is drawn once by the tile that owns it.
        let density = TilePipeline::new(
            "heatmap_density_pipeline".into(),
            *settings,
            shader.describe_vertex(),
            shader.describe_fragment(),
            TilePipelineOptions::default(),
        )
        .describe_render_pipeline()
        .initialize_with_prefix_layouts(device, &[projection_resources.as_ref()]);

        let shader = HeatmapCompositeShader {
            format: surface.surface_format(),
        };
        let mut composite = TilePipeline::new(
            "heatmap_composite_pipeline".into(),
            *settings,
            shader.describe_vertex(),
            shader.describe_fragment(),
            TilePipelineOptions {
                depth_stencil_enabled: true,
                // Always pass: the composite covers the viewport, not a tile.
                debug_stencil: true,
                multisampling: surface.is_multisampling_supported(settings.msaa),
                textured: true,
                ..Default::default()
            },
        )
        .describe_render_pipeline();
        composite
            .layout
            .get_or_insert_with(Vec::new)
            .push(ramp_layout());
        let composite = composite.initialize_with_prefix_layouts(device, &[]);
        HeatmapResources::new(device, density, composite)
    });
    Ok(())
}
