//! The day and night of a globe drawn without terrain, under the host's sun.
//!
//! Terrain turns its drape from day to night as it draws it. Without terrain every layer lies on
//! the globe itself, so one pass after the main pass multiplies what it drew by the share of its
//! colour each point of the globe keeps, as [`crate::celestial::TwilightBand`] and the share of
//! the sun an eclipse leaves set it. The pass runs before the translucent pass, so symbols, the
//! sun, the moon and the atmosphere keep their own light, as they do over terrain. It shades
//! only where the main pass wrote depth: the globe's background and anything standing on it,
//! not the sky or the stars behind the globe.

use bytemuck_derive::{Pod, Zeroable};

use crate::{
    context::MapContext,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        graph::{Node, NodeRunError, RenderContext, RenderGraphContext},
        projection::globe_camera_for_view,
        resource::{FragmentState, RenderPipeline, TilePipeline, TilePipelineOptions, VertexState},
        shaders::{Mat4x4f32, Shader},
        RenderResources,
    },
    tcs::{
        system::{SystemError, SystemResult},
        world::World,
    },
};

/// Draw-graph label of the pass, between the main and translucent passes.
pub const GLOBE_DAYLIGHT: &str = "globe_daylight";

/// The pass's pipeline.
pub struct GlobeDaylightPipeline(pub wgpu::RenderPipeline);

/// The frame's inputs to the pass, in view axes; none while it shades nothing.
#[derive(Default)]
pub struct GlobeDaylightGpu(Option<wgpu::Buffer>);

/// What the pass takes for one frame.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct GlobeDaylightFrame {
    inverse_projection: Mat4x4f32,
    /// The globe's centre in view axes and its radius, in pixels.
    globe: [f32; 4],
    /// The sun's direction in view axes.
    sun: [f32; 4],
    /// The twilight band and the share of the sun an eclipse leaves.
    twilight: [f32; 4],
    /// The moon's direction in view axes and the light it adds overhead.
    moonlight: [f32; 4],
}

/// The pass's shaders.
pub(crate) struct GlobeDaylightShader {
    pub(crate) format: wgpu::TextureFormat,
}

impl Shader for GlobeDaylightShader {
    fn describe_vertex(&self) -> VertexState {
        let vec4 = |index: u64, shader_location: u32| wgpu::VertexAttribute {
            offset: index * wgpu::VertexFormat::Float32x4.size(),
            format: wgpu::VertexFormat::Float32x4,
            shader_location,
        };
        VertexState {
            source: include_str!("../render/shaders/globe_daylight.vertex.wgsl"),
            entry_point: "main",
            buffers: vec![crate::render::resource::VertexBufferLayout {
                array_stride: std::mem::size_of::<GlobeDaylightFrame>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: (0..8).map(|index| vec4(index, index as u32)).collect(),
            }],
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        FragmentState {
            source: concat!(
                include_str!("../render/shaders/daylight.wgsl.part"),
                include_str!("../render/shaders/globe_daylight.fragment.wgsl")
            ),
            entry_point: "main",
            targets: vec![Some(wgpu::ColorTargetState {
                format: self.format,
                // The drawn colour, premultiplied, times the factor; its coverage unchanged.
                blend: Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::Dst,
                        dst_factor: wgpu::BlendFactor::Zero,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::Zero,
                        dst_factor: wgpu::BlendFactor::One,
                        operation: wgpu::BlendOperation::Add,
                    },
                }),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }
    }
}

/// Builds the pipeline once.
pub fn resource_system(
    MapContext {
        world, renderer, ..
    }: &mut MapContext,
) -> SystemResult {
    let Some(pipeline) = world
        .resources
        .get_mut::<Eventually<GlobeDaylightPipeline>>()
    else {
        return Err(SystemError::Dependencies);
    };
    let settings = renderer.settings;
    let surface = &renderer.resources.surface;
    pipeline.initialize(|| {
        let shader = GlobeDaylightShader {
            format: surface.surface_format(),
        };
        let mut descriptor = TilePipeline::new(
            "globe_daylight_pipeline".into(),
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
        .describe_render_pipeline();
        // The triangle lies at the far plane and passes only in front of what the main pass
        // drew at a nearer depth; the cleared sky keeps the far plane's depth.
        if let Some(depth) = descriptor.depth_stencil.as_mut() {
            depth.depth_write_enabled = Some(false);
            depth.depth_compare = Some(wgpu::CompareFunction::Less);
        }
        GlobeDaylightPipeline(descriptor.initialize(&renderer.device))
    });
    Ok(())
}

/// Prepares the frame's inputs while a host's sun lights a globe drawn without terrain.
pub fn queue_system(
    MapContext {
        world,
        style,
        view_state,
        renderer,
        ..
    }: &mut MapContext,
) -> SystemResult {
    let globe = style.projection.as_ref().is_some_and(|projection| {
        projection
            .projection_type
            .globe_transition(view_state.zoom().value())
            > 0.0
    });
    let lighting = crate::celestial::lighting(world);
    let frame = match lighting.sun {
        Some(sun) if globe && style.terrain.is_none() => {
            let camera = globe_camera_for_view(view_state).map_err(|error| {
                tracing::error!(?error, "the globe's daylight has no camera");
                SystemError::Setup
            })?;
            let moon = crate::celestial::discs(world).moon;
            let moonlight = crate::celestial::moonlight_uniform(Some(sun), moon);
            let in_view = |direction: cgmath::Vector3<f64>| {
                camera
                    .world_direction_to_view(direction)
                    .map(|view| [view.x as f32, view.y as f32, view.z as f32])
            };
            let moon_world = cgmath::Vector3::new(
                f64::from(moonlight[0]),
                f64::from(moonlight[1]),
                f64::from(moonlight[2]),
            );
            let (Some(sun), Some(moon)) = (
                in_view(sun.globe_world()),
                in_view(moon_world).or(Some([0.0; 3])),
            ) else {
                tracing::error!("the sun has no direction in view axes");
                return Err(SystemError::Setup);
            };
            let centre = camera.globe_center_in_view();
            let inverse_projection: cgmath::Matrix4<f32> = camera
                .inverse_projection()
                .cast()
                .ok_or(SystemError::Setup)?;
            Some(GlobeDaylightFrame {
                inverse_projection: inverse_projection.into(),
                globe: [
                    centre.x as f32,
                    centre.y as f32,
                    centre.z as f32,
                    camera.globe_radius_pixels() as f32,
                ],
                sun: [sun[0], sun[1], sun[2], 0.0],
                twilight: lighting.daylight_uniforms()[1],
                moonlight: [moon[0], moon[1], moon[2], moonlight[3]],
            })
        }
        _ => None,
    };
    let buffer = frame.map(|frame| {
        renderer.queue.create_buffer_init(
            &renderer.device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("globe daylight frame"),
                contents: bytemuck::bytes_of(&frame),
                usage: wgpu::BufferUsages::VERTEX,
            },
        )
    });
    world.resources.insert(GlobeDaylightGpu(buffer));
    Ok(())
}

/// Multiplies the frame's day and night into what the main pass drew.
pub struct GlobeDaylightNode;

impl Node for GlobeDaylightNode {
    fn run(
        &self,
        _graph: &mut RenderGraphContext,
        render_context: &mut RenderContext,
        state: &RenderResources,
        world: &World,
    ) -> Result<(), NodeRunError> {
        let (
            Some(GlobeDaylightGpu(Some(frame))),
            Some(Initialized(GlobeDaylightPipeline(pipeline))),
            Initialized(render_target),
            Initialized(multisampling),
            Initialized(depth),
        ) = (
            world.resources.get::<GlobeDaylightGpu>(),
            world.resources.get::<Eventually<GlobeDaylightPipeline>>(),
            &state.render_target,
            &state.multisampling_texture,
            &state.depth_texture,
        )
        else {
            return Ok(());
        };
        let load = wgpu::Operations {
            load: wgpu::LoadOp::Load,
            store: wgpu::StoreOp::Store,
        };
        let load_depth = wgpu::Operations {
            load: wgpu::LoadOp::Load,
            store: wgpu::StoreOp::Store,
        };
        // The translucent pass resolves the multisampled colour after it.
        let view = multisampling
            .as_ref()
            .map_or(&**render_target, |texture| &texture.view);
        let mut pass =
            render_context
                .command_encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("globe_daylight_pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: load,
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth.view,
                        depth_ops: Some(load_depth),
                        stencil_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        }),
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
        pass.set_pipeline(pipeline);
        pass.set_vertex_buffer(0, frame.slice(..));
        pass.draw(0..3, 0..1);
        Ok(())
    }
}

/// Puts the pass after the main pass and before the translucent pass.
pub(super) fn add_to_graph(graph: &mut crate::render::graph::RenderGraph) {
    use crate::render::draw_graph;
    let Some(draw_graph) = graph.get_sub_graph_mut(draw_graph::NAME) else {
        tracing::error!("draw graph is missing; the globe will not turn from day to night");
        return;
    };
    draw_graph.add_node(GLOBE_DAYLIGHT, GlobeDaylightNode);
    let edges = [
        draw_graph.add_node_edge(draw_graph::node::MAIN_PASS, GLOBE_DAYLIGHT),
        draw_graph.add_node_edge(GLOBE_DAYLIGHT, draw_graph::node::TRANSLUCENT_PASS),
    ];
    for edge in edges {
        if let Err(error) = edge {
            tracing::error!(?error, "the globe's daylight pass cannot be ordered");
        }
    }
}
