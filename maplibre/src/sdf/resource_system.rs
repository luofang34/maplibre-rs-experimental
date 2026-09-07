//! Prepares GPU-owned resources by initializing them if they are uninitialized or out-of-date.
use wgpu::util::{DeviceExt, TextureDataOrder};

use crate::{
    context::MapContext,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        resource::{RenderPipeline, TilePipeline},
        shaders,
        shaders::Shader,
        RenderResources, Renderer,
    },
    sdf::resource::SymbolTerrainFallback,
    sdf::{resource::GlyphTexture, text::GlyphSet, SymbolBufferPool, SymbolPipeline},
    tcs::system::{SystemError, SystemResult},
    terrain::resources::TerrainResources,
    vector::resource::BufferPool,
};

pub fn resource_system(
    MapContext {
        world,
        renderer:
            Renderer {
                device,
                queue,
                resources: RenderResources { surface, .. },
                settings,
                ..
            },
        ..
    }: &mut MapContext,
) -> SystemResult {
    let Some((
        symbol_buffer_pool,
        symbol_pipeline,
        glyph_texture_sampler,
        glyph_texture_bind_group,
        symbol_terrain_fallback,
        Initialized(projection_resources),
    )) = world.resources.query_mut::<(
        &mut Eventually<SymbolBufferPool>,
        &mut Eventually<SymbolPipeline>,
        &mut Eventually<(wgpu::Texture, wgpu::Sampler)>,
        &mut Eventually<GlyphTexture>,
        &mut Eventually<SymbolTerrainFallback>,
        &mut Eventually<ProjectionGpuResources>,
    )>()
    else {
        return Err(SystemError::Dependencies);
    };

    symbol_buffer_pool
        .initialize(|| BufferPool::from_device_with_sizes(device, settings.symbol_pools));

    symbol_pipeline.initialize(|| {
        let tile_shader = shaders::SymbolShader {
            format: surface.surface_format(),
        };

        let mut descriptor = TilePipeline::new(
            "symbol_pipeline".into(),
            *settings,
            tile_shader.describe_vertex(),
            tile_shader.describe_fragment(),
            false,
            false,
            true, // TODO ignore tile mask
            false,
            surface.is_multisampling_supported(settings.msaa),
            false,
            true,
        )
        .describe_render_pipeline();
        // Group 2 is the terrain tile the symbols stand on, so the vertex shader can lift
        // their anchors onto the surface as GL JS `get_elevation` does.
        descriptor
            .layout
            .get_or_insert_with(Vec::new)
            .push(TerrainResources::bind_group_layout_entries());
        let pipeline = descriptor
            .initialize_with_prefix_layouts(device, &[projection_resources.bind_group_layout()]);
        symbol_terrain_fallback.initialize(|| SymbolTerrainFallback::new(device, &pipeline));

        let (texture, sampler) = glyph_texture_sampler.initialize(|| {
            let data = include_bytes!("../../../data/0-255.pbf");
            let glyphs = GlyphSet::try_from(data.as_slice()).unwrap();

            let (width, height) = glyphs.get_texture_dimensions();

            let texture = device.create_texture_with_data(
                queue,
                &wgpu::TextureDescriptor {
                    label: Some("Glyph Texture"),
                    size: wgpu::Extent3d {
                        width: width as _,
                        height: height as _,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[wgpu::TextureFormat::R8Unorm], // TODO
                },
                TextureDataOrder::LayerMajor, // TODO
                glyphs.get_texture_bytes(),
            );

            let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                // SDF rendering requires linear interpolation
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            });

            (texture, sampler)
        });

        glyph_texture_bind_group.initialize(|| {
            GlyphTexture::from_device(device, texture, sampler, &pipeline.get_bind_group_layout(1))
        });

        SymbolPipeline(pipeline)
    });
    Ok(())
}
