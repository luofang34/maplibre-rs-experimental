//! The overlay's pipeline and the buffers holding the host's mesh.

use wgpu::util::BufferInitDescriptor;

use super::{HostOverlay, OverlayVertex};
use crate::{
    context::MapContext,
    render::{
        eventually::Eventually, settings::Msaa, upload_queue::UploadQueue, RenderResources,
        Renderer,
    },
    schedule::system::{SystemError, SystemResult},
    window::PhysicalSize,
};

const SHADER: &str = include_str!(concat!(
    env!("OUT_DIR"),
    "/wgsl/engine/render/gpu/shaders/overlay.wgsl"
));

/// Share by which a vertex's or probe's reversed-Z depth is raised before it is tested against
/// the frame's depth. That brings it nearer by about `DEPTH_BIAS * (1 - distance / far)` of
/// its distance from the camera: the full share close to the camera, less towards the far
/// plane. Overlay lines are straight chords between points on the ground, a few pixels apart,
/// so over a convex slope they run a little under the drawn surface; this keeps them from
/// hiding what lies on the ground, while a ridge hides what lies behind it by far more.
pub const DEPTH_BIAS: f32 = 0.01;

/// The mesh last uploaded: its reusable buffers, capacities, index count, and revision.
struct MeshBuffers {
    vertices: wgpu::Buffer,
    vertex_capacity: u64,
    indices: wgpu::Buffer,
    index_capacity: u64,
    count: u32,
    revision: u64,
}

impl Drop for MeshBuffers {
    fn drop(&mut self) {
        self.vertices.destroy();
        self.indices.destroy();
    }
}

const MIN_OVERLAY_BUFFER_BYTES: u64 = 4096;

/// What the overlay pass draws with.
pub(crate) struct OverlayGpu {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    viewport: wgpu::Buffer,
    size: Option<(u32, u32)>,
    mesh: Option<MeshBuffers>,
}

impl Drop for OverlayGpu {
    fn drop(&mut self) {
        self.mesh = None;
        self.viewport.destroy();
    }
}

fn overlay_viewport_binding(
    device: &wgpu::Device,
    queue: &UploadQueue,
) -> (wgpu::Buffer, wgpu::BindGroupLayout, wgpu::BindGroup) {
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
    (viewport, layout, bind_group)
}

fn overlay_render_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
    depth_format: wgpu::TextureFormat,
    samples: u32,
) -> wgpu::RenderPipeline {
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("overlay"),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    let module =
        crate::render::resource::create_shader_module_from_wgsl(device, Some("overlay"), SHADER);
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
    })
}

impl OverlayGpu {
    fn new(
        device: &wgpu::Device,
        queue: &UploadQueue,
        format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        samples: u32,
    ) -> Self {
        let (viewport, layout, bind_group) = overlay_viewport_binding(device, queue);
        let pipeline = overlay_render_pipeline(device, &layout, format, depth_format, samples);
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

    /// Uploads the host's mesh when it changed since the last upload, reusing GPU buffers
    /// whenever their capacity already fits the new geometry.
    fn upload(&mut self, device: &wgpu::Device, queue: &UploadQueue, overlay: &HostOverlay) {
        if self.mesh.as_ref().map(|m| m.revision) == Some(overlay.revision()) {
            return;
        }
        let mesh = overlay.mesh();
        // Whole triangles only: a trailing partial triangle would read past the mesh.
        let count = mesh.indices.len() - mesh.indices.len() % 3;
        if mesh.vertices.is_empty() || count == 0 {
            if let Some(existing) = self.mesh.as_mut() {
                existing.count = 0;
                existing.revision = overlay.revision();
            }
            return;
        }
        let v_bytes: &[u8] = bytemuck::cast_slice(&mesh.vertices);
        let i_bytes: &[u8] = bytemuck::cast_slice(mesh.indices.get(..count).unwrap_or_default());
        let grow = |needed: usize| -> u64 {
            (needed as u64)
                .max(MIN_OVERLAY_BUFFER_BYTES)
                .checked_next_power_of_two()
                .unwrap_or(needed as u64)
        };
        let alloc = |label, capacity: u64, usage: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: capacity,
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let buffers = match self.mesh.as_mut() {
            Some(existing) => {
                if v_bytes.len() as u64 > existing.vertex_capacity {
                    queue.cancel_staged_buffer(&existing.vertices);
                    existing.vertices.destroy();
                    existing.vertex_capacity = grow(v_bytes.len());
                    existing.vertices = alloc(
                        "overlay vertices",
                        existing.vertex_capacity,
                        wgpu::BufferUsages::VERTEX,
                    );
                }
                if i_bytes.len() as u64 > existing.index_capacity {
                    queue.cancel_staged_buffer(&existing.indices);
                    existing.indices.destroy();
                    existing.index_capacity = grow(i_bytes.len());
                    existing.indices = alloc(
                        "overlay indices",
                        existing.index_capacity,
                        wgpu::BufferUsages::INDEX,
                    );
                }
                existing
            }
            None => {
                let vertex_capacity = grow(v_bytes.len());
                let index_capacity = grow(i_bytes.len());
                self.mesh.insert(MeshBuffers {
                    vertices: alloc(
                        "overlay vertices",
                        vertex_capacity,
                        wgpu::BufferUsages::VERTEX,
                    ),
                    vertex_capacity,
                    indices: alloc("overlay indices", index_capacity, wgpu::BufferUsages::INDEX),
                    index_capacity,
                    count: 0,
                    revision: 0,
                })
            }
        };
        queue.write_buffer(&buffers.vertices, 0, v_bytes);
        queue.write_buffer(&buffers.indices, 0, i_bytes);
        buffers.count = count as u32;
        buffers.revision = overlay.revision();
    }

    #[inline]
    pub(super) fn has_mesh(&self) -> bool {
        self.mesh.is_some()
    }

    pub(super) fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        let Some(mesh) = &self.mesh else {
            return;
        };
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, mesh.vertices.slice(..));
        pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..mesh.count, 0, 0..1);
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
