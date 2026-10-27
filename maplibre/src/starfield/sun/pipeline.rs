//! The sun's shaders and render pipeline, and the depth copy sized to the frame.

use super::depth::SunDepth;
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

/// The sun's pipeline: projection at group 0, the frame at group 1, the copied depth at 2.
pub struct SunPipeline(pub wgpu::RenderPipeline);

struct SunShader {
    format: wgpu::TextureFormat,
}

impl Shader for SunShader {
    fn describe_vertex(&self) -> VertexState {
        VertexState {
            source: concat!(
                include_str!("../../render/shaders/projection.vertex.wgsl"),
                include_str!("sun.vertex.wgsl")
            ),
            entry_point: "main",
            buffers: Vec::new(),
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        FragmentState {
            source: include_str!("sun.fragment.wgsl"),
            entry_point: "main",
            targets: vec![Some(wgpu::ColorTargetState {
                format: self.format,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }
    }
}

/// Builds the pipeline once the projection resources exist, and keeps the depth copy the size
/// and sample count of the frame's depth.
pub fn resource_system(
    MapContext {
        world, renderer, ..
    }: &mut MapContext,
) -> SystemResult {
    let Some((pipeline, depth, Initialized(projection))) = world.resources.query_mut::<(
        &mut Eventually<SunPipeline>,
        &mut Eventually<SunDepth>,
        &Eventually<ProjectionGpuResources>,
    )>() else {
        return Err(SystemError::Dependencies);
    };
    let device = &renderer.device;
    let settings = renderer.settings;
    let surface = &renderer.resources.surface;
    let SunPipeline(pipeline) = pipeline.initialize(|| {
        let shader = SunShader {
            format: surface.surface_format(),
        };
        let layout = |label: &str, entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries,
            })
        };
        let frame_layout = layout(
            "sun frame layout",
            &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        );
        let depth_layout = layout("sun depth layout", &SunDepth::layout_entries());
        SunPipeline(
            TilePipeline::new(
                "sun_pipeline".into(),
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
            // The sun lies at the far plane, behind everything that writes depth.
            .with_depth_test()
            .describe_render_pipeline()
            .initialize_with_prefix_layouts(
                device,
                &[projection.bind_group_layout(), &frame_layout, &depth_layout],
            ),
        )
    });
    let Initialized(source) = &renderer.resources.depth_texture else {
        return Err(SystemError::Dependencies);
    };
    let size = source.texture.size();
    let samples = source.texture.sample_count();
    let depth_layout = pipeline.get_bind_group_layout(2);
    depth.reinitialize(
        || SunDepth::new(device, size, samples, &depth_layout),
        &(size.width, size.height, samples),
    );
    Ok(())
}
