//! Resolves frame depth for symbol occlusion and host compositors.
//!
//! A head-mounted display's compositor reprojects the frame for the moment it is shown and
//! blends it with the world, and needs the depth of every pixel to do so. The map renders
//! depth with a stencil, in whatever format the device favours; this pass copies it into the
//! `Depth32Float` texture the host provides, after everything has been drawn.

use std::sync::Mutex;

use wgpu::StoreOp;

use crate::{
    render::{
        eventually::Eventually::{self, Initialized},
        graph::{Node, RenderContext},
        RenderResources,
    },
    tcs::world::World,
};

struct CachedDepthBind {
    source: wgpu::Texture,
    _view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
}

/// The pipeline that resolves the map's depth into a single-sample depth texture.
pub struct DepthCopyPipeline {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    samples: u32,
    cached_bind: Mutex<Option<CachedDepthBind>>,
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
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("depth copy"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let vertex = crate::render::resource::create_shader_module_from_wgsl(
            device,
            Some("depth copy vertex"),
            include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/engine/render/gpu/shaders/depth_copy.vertex.wgsl"
            )),
        );
        let fragment = crate::render::resource::create_shader_module_from_wgsl(
            device,
            Some("depth copy fragment"),
            if multisampled {
                include_str!(concat!(
                    env!("OUT_DIR"),
                    "/wgsl/engine/render/gpu/shaders/depth_copy_multisampled.fragment.wgsl"
                ))
            } else {
                include_str!(concat!(
                    env!("OUT_DIR"),
                    "/wgsl/engine/render/gpu/shaders/depth_copy.fragment.wgsl"
                ))
            },
        );
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("depth copy"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &vertex,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &fragment,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                targets: &[],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: Self::TARGET_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        Self {
            pipeline,
            layout,
            samples,
            cached_bind: Mutex::new(None),
        }
    }

    /// Drops any cached source depth texture bind group so resized textures are released promptly.
    pub(crate) fn clear_cached_bind(&self) {
        let mut cached = crate::util::sync::lock_unpoisoned(&self.cached_bind);
        *cached = None;
    }
    /// Resolves the rendered depth into a single-sample target.
    pub(crate) fn copy(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source_texture: &wgpu::Texture,
        target: &wgpu::TextureView,
    ) {
        let bind_group = {
            let mut cached = crate::util::sync::lock_unpoisoned(&self.cached_bind);
            if let Some(entry) = cached.as_ref().filter(|e| e.source == *source_texture) {
                entry.bind_group.clone()
            } else {
                // The map's depth texture carries a stencil; a depth texture binding sees one aspect.
                let view = source_texture.create_view(&wgpu::TextureViewDescriptor {
                    aspect: wgpu::TextureAspect::DepthOnly,
                    ..Default::default()
                });
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("depth copy"),
                    layout: &self.layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    }],
                });
                *cached = Some(CachedDepthBind {
                    source: source_texture.clone(),
                    _view: view,
                    bind_group: bind_group.clone(),
                });
                bind_group
            }
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            multiview_mask: None,
            label: cfg!(debug_assertions).then_some("depth_copy"),
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
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

impl crate::render::eventually::HasChanged for DepthCopyPipeline {
    type Criteria = u32;

    fn has_changed(&self, criteria: &Self::Criteria) -> bool {
        self.samples != *criteria
    }
}

/// Copies the frame's depth into the host's depth texture, when the frame has one.
pub struct DepthCopyNode;

impl Node for DepthCopyNode {
    fn writes_custom_scene_depth(&self) -> bool {
        false
    }

    fn run(&self, render_context: &mut RenderContext, state: &RenderResources, world: &World) {
        let target: Option<&wgpu::TextureView> =
            state
                .eye_depth_target
                .as_ref()
                .or_else(|| match &state.depth_readout_texture {
                    Initialized(texture) => Some(&**texture),
                    Eventually::Uninitialized => None,
                });
        let Some(target) = target else {
            return;
        };
        let Initialized(depth_texture) = &state.depth_texture else {
            return;
        };
        let Some(Initialized(pipeline)) = world.resources.get::<Eventually<DepthCopyPipeline>>()
        else {
            return;
        };
        pipeline.copy(
            render_context.device,
            &mut render_context.command_encoder,
            depth_texture.as_ref(),
            target,
        );
    }
}

#[derive(Default)]
struct PersistentDepthChecks(Vec<fn(&World) -> bool>);

/// Registers a predicate checked by render preparation to retain a persistent depth attachment.
pub(crate) fn register_persistent_depth_check(world: &mut World, check: fn(&World) -> bool) {
    let checks = world.resources.get_or_init_mut::<PersistentDepthChecks>();
    if !checks.0.contains(&check) {
        checks.0.push(check);
    }
}

pub(crate) struct CachedDepthFormatFeatures {
    pub(crate) format: wgpu::TextureFormat,
    pub(crate) supports_transient: bool,
}

struct CachedDepthLayerScan {
    style_rev: u64,
    pitch_tilted: bool,
    needs_persistent: bool,
}

pub(crate) fn compute_needs_persistent_depth(
    world: &mut World,
    style: &crate::style::Style,
    view_state: &crate::render::view_state::ViewState,
    settings: &crate::render::settings::RendererSettings,
    state: &RenderResources,
    render_graph: &crate::render::graph::RenderGraph,
) -> bool {
    if !world
        .resources
        .get::<CachedDepthFormatFeatures>()
        .is_some_and(|c| c.supports_transient)
    {
        return true;
    }
    let world_needs = world
        .resources
        .get::<crate::render::ground::FrameGround>()
        .is_some_and(|g| g.0.is_some())
        || world
            .resources
            .get::<PersistentDepthChecks>()
            .is_some_and(|checks| checks.0.iter().any(|check| check(world)));
    if settings.depth_readout
        || state.eye_depth_target.is_some()
        || style.terrain.is_some()
        || world_needs
        || !render_graph.has_main_before_translucent()
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

pub(crate) fn depth_texture_usage(
    supports_transient: bool,
    needs_persistent: bool,
) -> wgpu::TextureUsages {
    if supports_transient && !needs_persistent {
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TRANSIENT_ATTACHMENT
    } else {
        // A host compositor or downstream pass reads depth back via copy or sampling.
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING
    }
}
