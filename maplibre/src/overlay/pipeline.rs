//! The overlay's pipeline and the buffers holding the host's mesh.

use wgpu::util::BufferInitDescriptor;

use super::{HostOverlay, OverlayVertex};
use crate::{
    context::MapContext,
    render::{
        eventually::Eventually, settings::Msaa, upload_queue::UploadQueue, RenderResources,
        Renderer,
    },
    tcs::system::{SystemError, SystemResult},
    window::PhysicalSize,
};

const SHADER: &str = include_str!("../render/shaders/overlay.wgsl");

/// Share of a vertex's distance from the camera it is brought nearer before it is tested
/// against the frame's depth. Overlay lines are straight chords between points on the
/// ground, a few pixels apart, so over a convex slope they run a little under the drawn
/// surface, and the ground under a point is sampled from the elevation tiles rather than
/// from the drawn mesh; this keeps both from hiding what lies on the ground, while a ridge
/// hides what lies behind it by far more.
pub const DEPTH_BIAS: f32 = 0.01;

/// The mesh last uploaded: its buffers and index count, and the revision they hold.
pub(super) struct MeshBuffers {
    pub(super) vertices: wgpu::Buffer,
    pub(super) indices: wgpu::Buffer,
    pub(super) count: u32,
    revision: u64,
}

/// What the overlay pass draws with.
pub(crate) struct OverlayGpu {
    pub(super) pipeline: wgpu::RenderPipeline,
    pub(super) bind_group: wgpu::BindGroup,
    viewport: wgpu::Buffer,
    size: Option<(u32, u32)>,
    pub(super) mesh: Option<MeshBuffers>,
}

impl OverlayGpu {
    fn new(
        device: &wgpu::Device,
        queue: &UploadQueue,
        format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        samples: u32,
    ) -> Self {
        let viewport = queue.create_buffer_init(
            device,
            &BufferInitDescriptor {
                label: Some("overlay viewport"),
                contents: bytemuck::cast_slice(&[1.0_f32, 1.0, DEPTH_BIAS, 0.0]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            },
        );
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: viewport.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("overlay"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlay"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("overlay"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<OverlayVertex>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32, 2 => Float32x4],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            // Tested as the map's 3D geometry is (reversed Z), never written: content of the
            // overlay does not hide other content of it.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: samples,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        });
        Self {
            pipeline,
            bind_group,
            viewport,
            size: None,
            mesh: None,
        }
    }

    /// Tells the shader the render target's size when it changes.
    fn set_size(&mut self, queue: &UploadQueue, size: &PhysicalSize) {
        let wanted = (size.width(), size.height());
        if self.size == Some(wanted) {
            return;
        }
        let data = [wanted.0 as f32, wanted.1 as f32, DEPTH_BIAS, 0.0];
        queue.write_buffer(&self.viewport, 0, bytemuck::cast_slice(&data));
        self.size = Some(wanted);
    }

    /// Uploads the host's mesh when it changed since the last upload.
    fn upload(&mut self, device: &wgpu::Device, queue: &UploadQueue, overlay: &HostOverlay) {
        if self.mesh.as_ref().map(|m| m.revision) == Some(overlay.revision()) {
            return;
        }
        let mesh = overlay.mesh();
        if mesh.is_empty() {
            self.mesh = None;
            return;
        }
        let buffer = |label, contents: &[u8], usage| {
            queue.create_buffer_init(
                device,
                &BufferInitDescriptor {
                    label: Some(label),
                    contents,
                    usage,
                },
            )
        };
        // Whole triangles only: a trailing partial triangle would read past the mesh.
        let count = mesh.indices.len() - mesh.indices.len() % 3;
        let indices = mesh.indices.get(..count).unwrap_or_default();
        self.mesh = Some(MeshBuffers {
            vertices: buffer(
                "overlay vertices",
                bytemuck::cast_slice(&mesh.vertices),
                wgpu::BufferUsages::VERTEX,
            ),
            indices: buffer(
                "overlay indices",
                bytemuck::cast_slice(indices),
                wgpu::BufferUsages::INDEX,
            ),
            count: count as u32,
            revision: overlay.revision(),
        });
    }
}

/// Builds the pipeline once the render target exists, then keeps the size and mesh current.
pub(super) fn prepare_system(
    MapContext {
        world,
        renderer:
            Renderer {
                device,
                queue,
                resources: RenderResources { surface, .. },
                settings,
                ..
            },
        ..
    }: &mut MapContext,
) -> SystemResult {
    let Some((gpu, overlay)) = world
        .resources
        .query_mut::<(&mut Eventually<OverlayGpu>, &HostOverlay)>()
    else {
        return Err(SystemError::Dependencies);
    };
    let samples = if surface.is_multisampling_supported(settings.msaa) {
        settings.msaa
    } else {
        Msaa { samples: 1 }
    };
    let gpu = gpu.initialize(|| {
        OverlayGpu::new(
            device,
            queue,
            surface.surface_format(),
            settings.depth_texture_format,
            samples.samples,
        )
    });
    gpu.set_size(queue, &surface.size());
    gpu.upload(device, queue, overlay);
    Ok(())
}
