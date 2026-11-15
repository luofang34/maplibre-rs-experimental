//! A snapshot of opaque depth for symbol-anchor visibility and per-frame fade clock uniform.

use crate::render::depth_copy::DepthCopyPipeline;

pub(crate) struct SymbolDepth {
    pub texture: wgpu::Texture,
    samples: u32,
    pub view: wgpu::TextureView,
    fade_buffer: wgpu::Buffer,
    last_fade_bits: Option<[u32; 2]>,
    pub binding: wgpu::BindGroup,
    pub copy: DepthCopyPipeline,
}

impl SymbolDepth {
    pub fn layout() -> Vec<wgpu::BindGroupLayoutEntry> {
        vec![
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Texture {
                    // An unfiltered float binding preserves raw depth reads on GLSL backends.
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: std::num::NonZeroU64::new(16),
                },
                count: None,
            },
        ]
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
        let fade_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("symbol fade clock"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("symbol anchor depth and fade"),
            layout: &pipeline.get_bind_group_layout(2),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: fade_buffer.as_entire_binding(),
                },
            ],
        });
        Self {
            texture,
            view,
            fade_buffer,
            last_fade_bits: None,
            binding,
            samples,
            copy: DepthCopyPipeline::new(device, samples),
        }
    }

    pub fn write_fade_time(
        &mut self,
        queue: &crate::render::upload_queue::UploadQueue,
        fade_time_ms: f32,
        fade_duration_ms: f32,
    ) {
        let bits = [fade_time_ms.to_bits(), fade_duration_ms.to_bits()];
        if self.last_fade_bits != Some(bits) {
            let payload = [fade_time_ms, fade_duration_ms, 0.0_f32, 0.0_f32];
            queue.write_buffer(&self.fade_buffer, 0, bytemuck::bytes_of(&payload));
            self.last_fade_bits = Some(bits);
        }
    }
}

impl crate::render::eventually::HasChanged for SymbolDepth {
    type Criteria = (u32, u32, u32);
    fn has_changed(&self, criteria: &Self::Criteria) -> bool {
        (self.texture.width(), self.texture.height(), self.samples) != *criteria
    }
}

impl Drop for SymbolDepth {
    fn drop(&mut self) {
        self.texture.destroy();
        self.fade_buffer.destroy();
    }
}
