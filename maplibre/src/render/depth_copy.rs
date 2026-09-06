//! Hands the frame's depth to a host compositor.
//!
//! A head-mounted display's compositor reprojects the frame for the moment it is shown and
//! blends it with the world, and needs the depth of every pixel to do so. The map renders
//! depth with a stencil, in whatever format the device favours; this pass copies it into the
//! `Depth32Float` texture the host provides, after everything has been drawn.

use wgpu::StoreOp;

use crate::{
    render::{
        eventually::Eventually::{self, Initialized},
        graph::{Node, NodeRunError, RenderContext, RenderGraphContext},
        RenderResources,
    },
    tcs::world::World,
};

/// The pipeline that writes the map's depth into a host's depth texture.
pub struct DepthCopyPipeline {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
}

impl DepthCopyPipeline {
    /// Format of the depth texture the host provides.
    pub const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

    /// Builds the pipeline for a source depth texture with `samples` samples per pixel.
    pub fn new(device: &wgpu::Device, samples: u32) -> Self {
        let multisampled = samples > 1;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("depth copy"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("depth copy"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let vertex = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("depth copy vertex"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/depth_copy.vertex.wgsl").into()),
        });
        let fragment = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("depth copy fragment"),
            source: wgpu::ShaderSource::Wgsl(
                if multisampled {
                    include_str!("shaders/depth_copy_multisampled.fragment.wgsl")
                } else {
                    include_str!("shaders/depth_copy.fragment.wgsl")
                }
                .into(),
            ),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("depth copy"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &vertex,
                entry_point: "main",
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &fragment,
                entry_point: "main",
                compilation_options: Default::default(),
                targets: &[],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: Self::TARGET_FORMAT,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Always,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        Self { pipeline, layout }
    }
}

/// Copies the frame's depth into the host's depth texture, when the frame has one.
pub struct DepthCopyNode;

impl Node for DepthCopyNode {
    fn run(
        &self,
        _graph: &mut RenderGraphContext,
        render_context: &mut RenderContext,
        state: &RenderResources,
        world: &World,
    ) -> Result<(), NodeRunError> {
        let Some(target) = &state.eye_depth_target else {
            return Ok(());
        };
        let Initialized(depth_texture) = &state.depth_texture else {
            return Ok(());
        };
        let Some(Initialized(pipeline)) = world.resources.get::<Eventually<DepthCopyPipeline>>()
        else {
            return Ok(());
        };
        // The map's depth texture carries a stencil; a depth texture binding sees one aspect.
        let source = depth_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor {
                aspect: wgpu::TextureAspect::DepthOnly,
                ..Default::default()
            });
        let bind_group = render_context
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("depth copy"),
                layout: &pipeline.layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&source),
                }],
            });
        let mut pass =
            render_context
                .command_encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("depth_copy"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: target,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(0.0),
                            store: StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
        pass.set_pipeline(&pipeline.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
        Ok(())
    }
}
