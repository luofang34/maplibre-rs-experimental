//! A snapshot of opaque depth for symbol-anchor visibility and per-frame fade clock uniform.

use crate::render::depth_copy::DepthCopyPipeline;

pub(crate) struct SymbolDepth {
    pub texture: wgpu::Texture,
    target_size: wgpu::Extent3d,
    samples: u32,
    allocated_full: bool,
    bind_group_layout: wgpu::BindGroupLayout,
    pub view: wgpu::TextureView,
    fade_buffer: wgpu::Buffer,
    last_fade_bits: Option<[u32; 3]>,
    pub skip_depth_copy: bool,
    pub binding: wgpu::BindGroup,
    pub copy: Option<DepthCopyPipeline>,
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
        bind_group_layout: wgpu::BindGroupLayout,
    ) -> Self {
        let placeholder_size = wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        };
        let (texture, view) = create_depth_target(device, placeholder_size);
        let fade_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("symbol fade clock"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let binding = create_bind_group(device, &bind_group_layout, &view, &fade_buffer);
        Self {
            texture,
            target_size: size,
            samples,
            allocated_full: false,
            bind_group_layout,
            view,
            fade_buffer,
            last_fade_bits: None,
            skip_depth_copy: true,
            binding,
            copy: None,
        }
    }

    fn ensure_allocated(&mut self, device: &wgpu::Device) {
        if self.allocated_full {
            return;
        }
        let (new_tex, new_view) = create_depth_target(device, self.target_size);
        let new_binding = create_bind_group(
            device,
            &self.bind_group_layout,
            &new_view,
            &self.fade_buffer,
        );
        let old_tex = std::mem::replace(&mut self.texture, new_tex);
        old_tex.destroy();
        self.view = new_view;
        self.binding = new_binding;
        if self.copy.is_none() {
            self.copy = Some(DepthCopyPipeline::new(device, self.samples));
        }
        self.allocated_full = true;
    }

    pub fn write_fade_time(
        &mut self,
        device: &wgpu::Device,
        queue: &crate::render::upload_queue::UploadQueue,
        fade_time_ms: f32,
        fade_duration_ms: f32,
        skip_depth_occlusion: bool,
    ) {
        if !skip_depth_occlusion {
            self.ensure_allocated(device);
        }
        self.skip_depth_copy = skip_depth_occlusion;
        let skip_flag = if skip_depth_occlusion {
            1.0_f32
        } else {
            0.0_f32
        };
        let bits = [
            fade_time_ms.to_bits(),
            fade_duration_ms.to_bits(),
            skip_flag.to_bits(),
        ];
        if self.last_fade_bits != Some(bits) {
            let payload = [fade_time_ms, fade_duration_ms, skip_flag, 0.0_f32];
            queue.write_buffer(&self.fade_buffer, 0, bytemuck::bytes_of(&payload));
            self.last_fade_bits = Some(bits);
        }
    }
}

fn create_depth_target(
    device: &wgpu::Device,
    size: wgpu::Extent3d,
) -> (wgpu::Texture, wgpu::TextureView) {
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
    (texture, view)
}

fn create_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    fade_buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("symbol anchor depth and fade"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: fade_buffer.as_entire_binding(),
            },
        ],
    })
}

impl crate::render::eventually::HasChanged for SymbolDepth {
    type Criteria = (u32, u32, u32);
    fn has_changed(&self, criteria: &Self::Criteria) -> bool {
        (
            self.target_size.width,
            self.target_size.height,
            self.samples,
        ) != *criteria
    }
}

impl Drop for SymbolDepth {
    fn drop(&mut self) {
        self.texture.destroy();
        self.fade_buffer.destroy();
    }
}
