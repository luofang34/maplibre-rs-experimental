//! The moon's shaders and render pipeline.

use crate::{
    context::MapContext,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        resource::{FragmentState, RenderPipeline, TilePipeline, TilePipelineOptions, VertexState},
        shaders::Shader,
    },
    tcs::system::{SystemError, SystemResult},
};

/// The moon's pipeline: projection at group 0, the frame at group 1.
pub struct MoonPipeline(pub wgpu::RenderPipeline);

struct MoonShader {
    format: wgpu::TextureFormat,
}

impl Shader for MoonShader {
    fn describe_vertex(&self) -> VertexState {
        VertexState {
            source: concat!(
                include_str!("../../render/shaders/projection.vertex.wgsl"),
                include_str!("moon.vertex.wgsl")
            ),
            entry_point: "main",
            buffers: Vec::new(),
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        FragmentState {
            source: include_str!("moon.fragment.wgsl"),
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
pub fn resource_system(
    MapContext {
        world, renderer, ..
    }: &mut MapContext,
) -> SystemResult {
    let Some((pipeline, Initialized(projection))) = world.resources.query_mut::<(
        &mut Eventually<MoonPipeline>,
        &Eventually<ProjectionGpuResources>,
    )>() else {
        return Err(SystemError::Dependencies);
    };
    let device = &renderer.device;
    let settings = renderer.settings;
    let surface = &renderer.resources.surface;
    pipeline.initialize(|| {
        let shader = MoonShader {
            format: surface.surface_format(),
        };
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("moon frame layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        MoonPipeline(
            TilePipeline::new(
                "moon_pipeline".into(),
                settings,
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
            // The moon lies at the far plane, behind everything that writes depth.
            .with_depth_test()
            .describe_render_pipeline()
            .initialize_with_prefix_layouts(
                device,
                &[projection.bind_group_layout(), &frame_layout],
            ),
        )
    });
    Ok(())
}
