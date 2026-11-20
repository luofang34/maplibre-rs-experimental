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
                    render_graph,
                    ..
                },
            world,
            style,
            view_state,
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
        let world_needs_persistent_depth = world
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
        let supports_transient = adapter
            .get_texture_format_features(settings.depth_texture_format)
            .allowed_usages
            .contains(wgpu::TextureUsages::TRANSIENT_ATTACHMENT);
        let pitch_tilted = view_state.camera().get_pitch().0.abs() >= 1e-4;
        let needs_persistent_depth = settings.depth_readout
            || state.eye_depth_target.is_some()
            || style.terrain.is_some()
            || world_needs_persistent_depth
            || render_graph.has_custom_scene_depth_writers()
            || render_graph.contains("overlay_pass")
            || render_graph.contains("depth_probe_pass")
            || render_graph.contains("sun_depth_copy")
            || matches!(
                style.projection.as_ref().map(|p| &p.projection_type),
                Some(crate::projection::ProjectionType::VerticalPerspective)
            )
            || style.layers.iter().any(|l| {
                l.type_ == "custom"
                    || (pitch_tilted
                        && matches!(
                            l.type_.as_str(),
                            "fill-extrusion" | "raster" | "hillshade" | "color-relief"
                        ))
            });
        let depth_usage = if supports_transient && !needs_persistent_depth {
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TRANSIENT_ATTACHMENT
        } else {
            // A host compositor or downstream pass reads depth back via copy or sampling.
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING
        };
        if matches!(
            &state.depth_texture,
            Eventually::Initialized(tex) if tex.texture.usage() != depth_usage
        ) {
            state.depth_texture.take();
        }
        state.depth_texture.reinitialize(
            || {
                Texture::new(
                    Some("depth texture"),
                    device,
                    settings.depth_texture_format,
                    size.width(),
                    size.height(),
                    depth_msaa,
                    depth_usage,
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
            .with_buffer_destroyer(wgpu::Buffer::destroy)
        });

        let transition = style.projection.as_ref().map_or(0.0, |spec| {
            spec.projection_type
                .globe_transition(view_state.zoom().value())
        });
        let want_globe = transition > 0.0;
        let want_flat = transition < 1.0 || style.terrain.is_some();
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
        if matches!(mask_pipeline, Eventually::Uninitialized) || need_flat || need_globe {
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
                    .initialize_with_prefix_layouts(
                        device,
                        &[projection_resources.bind_group_layout()],
                    ),
                );
            }
            if need_globe {
                let globe_shader = shaders::TileMaskShader {
                    format: surface.surface_format(),
                    draw_colors: false,
                    debug_lines: false,
                };
                mask.globe = Some(
                    TilePipeline::new(
                        "mask_pipeline".into(),
                        *settings,
                        globe_shader.describe_vertex(),
                        globe_shader.describe_fragment(),
                        options,
                    )
                    .describe_render_pipeline()
                    .initialize_with_prefix_layouts(
                        device,
                        &[projection_resources.bind_group_layout()],
                    ),
                );
            }
        }

        Ok(())
    }
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
