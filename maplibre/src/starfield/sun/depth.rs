//! A copy of the frame's depth after the main pass, which the sun's glare reads to learn how
//! much of the disc the ground hides.

use crate::{
    render::{
        depth_copy::DepthCopyPipeline,
        eventually::{Eventually, Eventually::Initialized, HasChanged},
        graph::{Node, NodeRunError, RenderContext, RenderGraphContext},
        RenderResources,
    },
    tcs::world::World,
};

/// The draw graph's node copying the depth, between the main and translucent passes.
pub const SUN_DEPTH_COPY: &str = "sun_depth_copy";

/// The copied depth and the bind group through which the sun's shader reads it.
pub struct SunDepth {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    samples: u32,
    /// Bind group 2 of the sun's pipeline.
    pub binding: wgpu::BindGroup,
    copy: DepthCopyPipeline,
}

impl SunDepth {
    /// The bind group layout of the copied depth, as the sun's vertex shader reads it.
    pub fn layout_entries() -> [wgpu::BindGroupLayoutEntry; 1] {
        [wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Texture {
                // An unfiltered float binding preserves raw depth reads on GLSL backends.
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }]
    }

    /// A copy target of `size` for a depth texture of `samples` samples per pixel.
    pub fn new(
        device: &wgpu::Device,
        size: wgpu::Extent3d,
        samples: u32,
        layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sun depth"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DepthCopyPipeline::TARGET_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sun depth"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            }],
        });
        Self {
            texture,
            view,
            samples,
            binding,
            copy: DepthCopyPipeline::new(device, samples),
        }
    }
}

impl HasChanged for SunDepth {
    type Criteria = (u32, u32, u32);

    fn has_changed(&self, criteria: &Self::Criteria) -> bool {
        (self.texture.width(), self.texture.height(), self.samples) != *criteria
    }
}

/// Copies the depth the main pass left, while a sun is drawn.
pub struct SunDepthCopyNode;

impl Node for SunDepthCopyNode {
    fn run(
        &self,
        _graph: &mut RenderGraphContext,
        render_context: &mut RenderContext,
        state: &RenderResources,
        world: &World,
    ) -> Result<(), NodeRunError> {
        let drawn = world
            .resources
            .get::<super::SunGpu>()
            .is_some_and(|gpu| gpu.drawable().is_some());
        let (Some(Initialized(depth)), Initialized(source), true) = (
            world.resources.get::<Eventually<SunDepth>>(),
            &state.depth_texture,
            drawn,
        ) else {
            return Ok(());
        };
        depth.copy.copy(
            render_context.device,
            &mut render_context.command_encoder,
            &source.texture,
            &depth.view,
        );
        Ok(())
    }
}
