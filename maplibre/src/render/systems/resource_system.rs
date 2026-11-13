//! Prepares GPU-owned resources by initializing them if they are uninitialized or out-of-date.

use std::{borrow::Cow, mem};

use crate::{
    context::MapContext,
    render::{
        depth_copy::DepthCopyPipeline,
        eventually::Eventually,
        projection::ProjectionGpuResources,
        resource::{BackingBufferDescriptor, RenderPipeline, Texture, TilePipeline},
        settings::Msaa,
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
                    ..
                },
            world,
            ..
        }: &mut MapContext,
    ) -> SystemResult {
        if world
            .resources
            .get::<crate::render::tracked_pass::DrawCapabilities>()
            .is_none()
        {
            world.resources.insert(
                crate::render::tracked_pass::DrawCapabilities::from_adapter_and_device(
                    adapter, device,
                ),
            );
        }
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

        let surface = &mut state.surface;

        surface.reconfigure(device);

        let size = surface.size();

        let projection_resources =
            projection_resources.initialize(|| ProjectionGpuResources::new(device, queue));

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
        state.depth_texture.reinitialize(
            || {
                Texture::new(
                    Some("depth texture"),
                    device,
                    settings.depth_texture_format,
                    size.width(),
                    size.height(),
                    depth_msaa,
                    // A host compositor reads the depth back through the depth copy pass.
                    wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                )
            },
            &(size.width(), size.height()),
        );
        if settings.depth_readout {
            state.depth_readout_texture.reinitialize(
                || {
                    Texture::new(
                        Some("depth readout texture"),
                        device,
                        DepthCopyPipeline::TARGET_FORMAT,
                        size.width(),
                        size.height(),
                        Msaa { samples: 1 },
                        wgpu::TextureUsages::RENDER_ATTACHMENT
                            | wgpu::TextureUsages::TEXTURE_BINDING
                            | wgpu::TextureUsages::COPY_SRC,
                    )
                },
                &(size.width(), size.height()),
            );
        } else {
            state.depth_readout_texture.take();
        }
        if state.eye_depth_target.is_some() || settings.depth_readout {
            depth_copy.reinitialize(
                || DepthCopyPipeline::new(device, depth_msaa.samples),
                &depth_msaa.samples,
            );
        }

        state.multisampling_texture.reinitialize(
            || {
                if settings.msaa.is_multisampling()
                    && surface.is_multisampling_supported(settings.msaa)
                {
                    Some(Texture::new(
                        Some("multisampling texture"),
                        device,
                        surface.surface_format(),
                        size.width(),
                        size.height(),
                        settings.msaa,
                        wgpu::TextureUsages::RENDER_ATTACHMENT,
                    ))
                } else {
                    None
                }
            },
            &(size.width(), size.height()),
        );

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
        });

        mask_pipeline.initialize(|| {
            let options = crate::render::resource::TilePipelineOptions {
                depth_stencil_enabled: true,
                update_stencil: true,
                debug_stencil: false,
                wireframe: false,
                multisampling: surface.is_multisampling_supported(settings.msaa),
                textured: false,
            };
            let flat_shader = shaders::FlatTileMaskShader {
                format: surface.surface_format(),
            };
            let flat = TilePipeline::new(
                "mask_pipeline_flat".into(),
                *settings,
                flat_shader.describe_vertex(),
                flat_shader.describe_fragment(),
                options,
            )
            .describe_render_pipeline()
            .initialize_with_prefix_layouts(device, &[projection_resources.bind_group_layout()]);

            let globe_shader = shaders::TileMaskShader {
                format: surface.surface_format(),
                draw_colors: false,
                debug_lines: false,
            };
            let globe = TilePipeline::new(
                "mask_pipeline".into(),
                *settings,
                globe_shader.describe_vertex(),
                globe_shader.describe_fragment(),
                options,
            )
            .describe_render_pipeline()
            .initialize_with_prefix_layouts(device, &[projection_resources.bind_group_layout()]);

            MaskPipeline { flat, globe }
        });

        Ok(())
    }
}
