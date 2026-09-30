//! Backgrounds that repeat an image: the pipeline and the view they are drawn against.

use wgpu::util::DeviceExt;

use crate::render::view_state::ViewState;

/// The uniform of the view the pattern fragment shader reads; the layout mirrors the WGSL
/// struct.
#[repr(C)]
#[derive(Copy, Clone, bytemuck_derive::Pod, bytemuck_derive::Zeroable)]
struct ViewUniforms {
    clip_to_map: [[f32; 4]; 4],
    center: [f32; 4],
    viewport: [f32; 4],
}

/// The pipeline of pattern backgrounds and the uniform that places the pattern on the map.
pub(crate) struct BackgroundPatternGpu {
    pipeline: wgpu::RenderPipeline,
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

fn view_layout_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
    vec![wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }]
}

/// The bind group layouts of the pattern pipeline: the image, then the view.
pub(crate) fn layouts(device: &wgpu::Device) -> [wgpu::BindGroupLayout; 2] {
    [
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("background pattern image"),
            entries: &crate::vector::pattern::layout_entries(),
        }),
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("background pattern view"),
            entries: &view_layout_entries(),
        }),
    ]
}

impl BackgroundPatternGpu {
    pub(crate) fn new(
        device: &wgpu::Device,
        pipeline: wgpu::RenderPipeline,
        view_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("background pattern view"),
            contents: bytemuck::bytes_of(&<ViewUniforms as bytemuck::Zeroable>::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("background pattern view"),
            layout: view_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        Self {
            pipeline,
            buffer,
            bind_group,
        }
    }

    /// Points the pattern at the map the view looks at, as far as the frame can see it.
    pub(crate) fn write(&self, queue: &wgpu::Queue, view: &ViewState) {
        use cgmath::{Matrix4, SquareMatrix, Vector3};

        let center = view.camera().position();
        // Relative to the centre of the view the map coordinates are small, which keeps f32
        // exact enough; the centre itself reaches the shader as an exact integer split.
        let relative = view.view_projection().0
            * Matrix4::from_translation(Vector3::new(center.x, center.y, 0.0));
        let Some(inverse) = relative.invert() else {
            return;
        };
        let whole = center.x.floor();
        let whole_y = center.y.floor();
        let split = |value: f64, fraction: f64| {
            let upper = (value / 65536.0).floor();
            [upper as f32, (value - upper * 65536.0 + fraction) as f32]
        };
        let [upper_x, lower_x] = split(whole, center.x - whole);
        let [upper_y, lower_y] = split(whole_y, center.y - whole_y);
        let columns: [[f64; 4]; 4] = inverse.into();
        let uniforms = ViewUniforms {
            clip_to_map: columns.map(|column| column.map(|value| value as f32)),
            center: [upper_x, upper_y, lower_x, lower_y],
            viewport: [view.width() as f32, view.height() as f32, 0.0, 0.0],
        };
        queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(&uniforms));
    }

    pub(crate) fn pipeline(&self) -> &wgpu::RenderPipeline {
        &self.pipeline
    }

    pub(crate) fn view(&self) -> &wgpu::BindGroup {
        &self.bind_group
    }
}
