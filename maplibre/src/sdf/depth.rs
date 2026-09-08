//! A snapshot of opaque depth for symbol-anchor visibility.
use crate::render::depth_copy::DepthCopyPipeline;

pub(crate) struct SymbolDepth {
    pub texture: wgpu::Texture,
    samples: u32,
    pub view: wgpu::TextureView,
    pub binding: wgpu::BindGroup,
    pub copy: DepthCopyPipeline,
}

impl SymbolDepth {
    pub fn layout() -> Vec<wgpu::BindGroupLayoutEntry> {
        vec![wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }]
    }

    pub fn new(
        device: &wgpu::Device,
        size: wgpu::Extent3d,
        samples: u32,
        pipeline: &wgpu::RenderPipeline,
    ) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("symbol anchor depth"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("symbol anchor depth"),
            layout: &pipeline.get_bind_group_layout(2),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            }],
        });
        Self {
            texture,
            view,
            binding,
            samples,
            copy: DepthCopyPipeline::new(device, samples),
        }
    }
}

impl crate::render::eventually::HasChanged for SymbolDepth {
    type Criteria = (u32, u32, u32);
    fn has_changed(&self, criteria: &Self::Criteria) -> bool {
        (self.texture.width(), self.texture.height(), self.samples) != *criteria
    }
}
