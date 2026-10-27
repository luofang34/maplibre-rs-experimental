//! The starfield's shaders and render pipeline.

use crate::{
    context::MapContext,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        resource::{
            FragmentState, RenderPipeline, TilePipeline, TilePipelineOptions, VertexBufferLayout,
            VertexState,
        },
        shaders::Shader,
    },
    tcs::system::{SystemError, SystemResult},
};

/// The starfield's render pipeline: projection at group 0, the frame's values at group 1.
pub struct StarfieldPipeline(pub wgpu::RenderPipeline);

struct StarfieldShader {
    format: wgpu::TextureFormat,
}

impl Shader for StarfieldShader {
    fn describe_vertex(&self) -> VertexState {
        VertexState {
            source: concat!(
                include_str!("../render/shaders/projection.vertex.wgsl"),
                include_str!("../render/shaders/atmosphere_scattering.wgsl.part"),
                include_str!("starfield.vertex.wgsl")
            ),
            entry_point: "main",
            buffers: vec![VertexBufferLayout {
                array_stride: std::mem::size_of::<super::StarInstance>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: vec![
                    wgpu::VertexAttribute {
                        offset: 0,
                        format: wgpu::VertexFormat::Float32x4,
                        shader_location: 0,
                    },
                    wgpu::VertexAttribute {
                        offset: wgpu::VertexFormat::Float32x4.size(),
                        format: wgpu::VertexFormat::Float32x4,
                        shader_location: 1,
                    },
                ],
            }],
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        FragmentState {
            source: include_str!("starfield.fragment.wgsl"),
            entry_point: "main",
            targets: vec![Some(wgpu::ColorTargetState {
                format: self.format,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }
    }
}

/// Builds the pipeline once the projection resources exist.
pub(super) fn resource_system(
    MapContext {
        world,
        renderer:
            crate::render::Renderer {
                device,
                resources: crate::render::RenderResources { surface, .. },
                settings,
                ..
            },
        ..
    }: &mut MapContext,
) -> SystemResult {
    let Some((pipeline, Initialized(projection))) = world.resources.query_mut::<(
        &mut Eventually<StarfieldPipeline>,
        &Eventually<ProjectionGpuResources>,
    )>() else {
        return Err(SystemError::Dependencies);
    };
    pipeline.initialize(|| {
        let shader = StarfieldShader {
            format: surface.surface_format(),
        };
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("starfield frame layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let pipeline = TilePipeline::new(
            "starfield_pipeline".into(),
            *settings,
            shader.describe_vertex(),
            shader.describe_fragment(),
            TilePipelineOptions {
                depth_stencil_enabled: true,
                update_stencil: false,
                debug_stencil: true,
                wireframe: false,
                multisampling: surface.is_multisampling_supported(settings.msaa),
                textured: false,
            },
        )
        // The stars lie at the far plane, behind everything that writes depth.
        .with_depth_test()
        .describe_render_pipeline()
        .initialize_with_prefix_layouts(device, &[projection.bind_group_layout(), &frame_layout]);
        StarfieldPipeline(pipeline)
    });
    Ok(())
}
