//! Backgrounds that repeat an image: the pipeline and the view they are drawn against.

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
    globe: Option<GlobePattern>,
}

/// The pipeline of pattern backgrounds on the globe and the size of the world it repeats over.
struct GlobePattern {
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
        queue: &crate::render::upload_queue::UploadQueue,
        pipeline: wgpu::RenderPipeline,
        view_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let buffer = queue.create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("background pattern view"),
                contents: bytemuck::bytes_of(&<ViewUniforms as bytemuck::Zeroable>::zeroed()),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            },
        );
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
            globe: None,
        }
    }

    /// Adds the globe pipeline, whose third bind group is the size of the world.
    pub(crate) fn with_globe(
        &mut self,
        device: &wgpu::Device,
        queue: &crate::render::upload_queue::UploadQueue,
        pipeline: wgpu::RenderPipeline,
        world_layout: &wgpu::BindGroupLayout,
    ) {
        let buffer = queue.create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("globe background pattern world"),
                contents: bytemuck::bytes_of(&[1.0_f32; 4]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            },
        );
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globe background pattern world"),
            layout: world_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        self.globe = Some(GlobePattern {
            pipeline,
            buffer,
            bind_group,
        });
    }

    /// Whether the globe pipeline has been added.
    pub(crate) fn has_globe(&self) -> bool {
        self.globe.is_some()
    }

    /// The globe pipeline and the bind group of the world's size.
    pub(crate) fn globe(&self) -> Option<(&wgpu::RenderPipeline, &wgpu::BindGroup)> {
        self.globe
            .as_ref()
            .map(|globe| (&globe.pipeline, &globe.bind_group))
    }

    /// Points the pattern at the map the view looks at, as far as the frame can see it.
    pub(crate) fn write(
        &self,
        queue: &crate::render::upload_queue::UploadQueue,
        view: &ViewState,
        physical: [f32; 2],
    ) {
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
            // The shader reads pixel positions of the frame, which may have more pixels than
            // the view has logical ones.
            viewport: [physical[0], physical[1], 0.0, 0.0],
        };
        queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(&uniforms));
        if let Some(globe) = &self.globe {
            // Map pixels the world spans at the view's zoom, which the pattern repeats over.
            let world = (crate::coords::TILE_SIZE * 2_f64.powf(view.zoom().value())) as f32;
            queue.write_buffer(
                &globe.buffer,
                0,
                bytemuck::bytes_of(&[world, 0.0, 0.0, 0.0]),
            );
        }
    }

    pub(crate) fn pipeline(&self) -> &wgpu::RenderPipeline {
        &self.pipeline
    }

    pub(crate) fn view(&self) -> &wgpu::BindGroup {
        &self.bind_group
    }
}

/// The bind group layout of the world-size uniform the globe pattern reads.
pub(crate) fn world_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("globe background pattern world"),
        entries: &view_layout_entries(),
    })
}
