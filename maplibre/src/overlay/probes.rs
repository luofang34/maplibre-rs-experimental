//! Depth probes: whether points a host draws over the map, outside its frame (text, icons),
//! lie behind what the frame drew there.
//!
//! The probes go to the GPU with a frame, which compares each against the frame's depth at
//! its pixel, as the overlay pass compares its vertices; the answers are read back without
//! waiting, and arrive a frame or two later. One request is in flight at a time; probes set
//! meanwhile wait for it, the latest replacing earlier ones.

use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc,
};

use wgpu::util::BufferInitDescriptor;

use super::{DepthProbe, HostOverlay, ProbeResults, DEPTH_BIAS};
use crate::{
    context::MapContext,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        graph::{Node, NodeRunError, RenderContext, RenderGraphContext},
        settings::Msaa,
        upload_queue::UploadQueue,
        RenderResources, Renderer,
    },
    tcs::{
        system::{SystemError, SystemResult},
        world::World,
    },
};

/// The probe pass's node in the draw graph, after the overlay pass.
pub const DEPTH_PROBE_PASS: &str = "depth_probe_pass";

/// Probes answered at once; more are left unanswered.
pub const MAX_PROBES: usize = 65_536;

/// Texels per row of the result target: one row of bytes is the copy alignment.
const WIDTH: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;

const TO_ENCODE: u8 = 0;
const ENCODED: u8 = 1;
const MAPPING: u8 = 2;
const MAPPED: u8 = 3;
const FAILED: u8 = 4;

/// A request on its way through the GPU.
struct Pending {
    request: u64,
    count: u32,
    vertices: wgpu::Buffer,
    uniform: wgpu::Buffer,
    target: wgpu::Texture,
    readback: wgpu::Buffer,
    state: Arc<AtomicU8>,
}

/// The probe pipeline and the request in flight.
pub(crate) struct ProbeGpu {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    pending: Option<Pending>,
    sent: u64,
}

impl ProbeGpu {
    fn new(device: &wgpu::Device, samples: u32) -> Self {
        let multisampled = samples > 1;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("depth probes"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("depth probes"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let source = if multisampled {
            include_str!("../render/shaders/depth_probe_multisampled.wgsl")
        } else {
            include_str!("../render/shaders/depth_probe.wgsl")
        };
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("depth probes"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("depth probes"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<DepthProbe>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::PointList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        Self {
            pipeline,
            layout,
            pending: None,
            sent: 0,
        }
    }

    /// Takes the answers of the request in flight once they are back.
    fn collect(&mut self, device: &wgpu::Device) -> Option<ProbeResults> {
        let pending = self.pending.as_ref()?;
        if pending.state.load(Ordering::Acquire) == MAPPING {
            // Drives the mapping callback without waiting for the GPU.
            if let Err(error) = device.poll(wgpu::PollType::Poll) {
                tracing::debug!(%error, "polling for depth probes failed");
            }
        }
        match pending.state.load(Ordering::Acquire) {
            MAPPED => {}
            FAILED => {
                tracing::warn!(request = pending.request, "depth probes were not read back");
                self.pending = None;
                return None;
            }
            _ => return None,
        }
        let pending = self.pending.take()?;
        let visible = pending
            .readback
            .get_mapped_range(..)
            .ok()
            .map(|bytes| {
                bytes
                    .iter()
                    .take(pending.count as usize)
                    .map(|&byte| byte > 127)
                    .collect()
            })
            .unwrap_or_default();
        pending.readback.unmap();
        Some(ProbeResults {
            request: pending.request,
            visible,
        })
    }

    /// Puts the latest probes on their way when nothing is in flight.
    fn send(&mut self, device: &wgpu::Device, queue: &UploadQueue, overlay: &HostOverlay) {
        let (request, probes) = overlay.probes();
        if self.pending.is_some() || request == self.sent || probes.is_empty() {
            return;
        }
        self.sent = request;
        let probes = probes
            .get(..probes.len().min(MAX_PROBES))
            .unwrap_or_default();
        let count = probes.len() as u32;
        let rows = count.div_ceil(WIDTH);
        let vertices = queue.create_buffer_init(
            device,
            &BufferInitDescriptor {
                label: Some("depth probes"),
                contents: bytemuck::cast_slice(probes),
                usage: wgpu::BufferUsages::VERTEX,
            },
        );
        let uniform = queue.create_buffer_init(
            device,
            &BufferInitDescriptor {
                label: Some("depth probe target"),
                contents: bytemuck::cast_slice(&[WIDTH as f32, rows as f32, DEPTH_BIAS, 0.0]),
                usage: wgpu::BufferUsages::UNIFORM,
            },
        );
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("depth probe answers"),
            size: wgpu::Extent3d {
                width: WIDTH,
                height: rows,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("depth probe readback"),
            size: u64::from(WIDTH * rows),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        self.pending = Some(Pending {
            request,
            count,
            vertices,
            uniform,
            target,
            readback,
            state: Arc::new(AtomicU8::new(TO_ENCODE)),
        });
    }

    /// Asks for the answers once the frame that computes them has been submitted.
    pub(crate) fn after_submit(&self) {
        let Some(pending) = &self.pending else { return };
        if pending
            .state
            .compare_exchange(ENCODED, MAPPING, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let state = pending.state.clone();
        pending
            .readback
            .map_async(wgpu::MapMode::Read, .., move |result| {
                state.store(
                    if result.is_ok() { MAPPED } else { FAILED },
                    Ordering::Release,
                );
            });
    }
}

/// Builds the pipeline once the render target exists; collects answers and sends probes.
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
        .query_mut::<(&mut Eventually<ProbeGpu>, &mut HostOverlay)>()
    else {
        return Err(SystemError::Dependencies);
    };
    let samples = if surface.is_multisampling_supported(settings.msaa) {
        settings.msaa
    } else {
        Msaa { samples: 1 }
    };
    let gpu = gpu.initialize(|| ProbeGpu::new(device, samples.samples));
    if let Some(results) = gpu.collect(device) {
        overlay.answer(results);
    }
    gpu.send(device, queue, overlay);
    Ok(())
}

/// Compares the probes on their way with the finished frame's depth.
pub(super) struct DepthProbePassNode;

impl Node for DepthProbePassNode {
    fn run(
        &self,
        _graph: &mut RenderGraphContext,
        render_context: &mut RenderContext,
        state: &RenderResources,
        world: &World,
    ) -> Result<(), NodeRunError> {
        let Some(Initialized(gpu)) = world.resources.get::<Eventually<ProbeGpu>>() else {
            return Ok(());
        };
        let Some(pending) = &gpu.pending else {
            return Ok(());
        };
        if pending.state.load(Ordering::Acquire) != TO_ENCODE {
            return Ok(());
        }
        let Initialized(depth_texture) = &state.depth_texture else {
            return Ok(());
        };
        // The map's depth carries a stencil; a depth texture binding sees one aspect.
        let depth = depth_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor {
                aspect: wgpu::TextureAspect::DepthOnly,
                ..Default::default()
            });
        let bind_group = render_context
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("depth probes"),
                layout: &gpu.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: pending.uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&depth),
                    },
                ],
            });
        let answers = pending.target.create_view(&Default::default());
        {
            let mut pass =
                render_context
                    .command_encoder
                    .begin_render_pass(&wgpu::RenderPassDescriptor {
                        multiview_mask: None,
                        label: Some(DEPTH_PROBE_PASS),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            depth_slice: None,
                            view: &answers,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });
            pass.set_pipeline(&gpu.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.set_vertex_buffer(0, pending.vertices.slice(..));
            pass.draw(0..pending.count, 0..1);
        }
        render_context.command_encoder.copy_texture_to_buffer(
            pending.target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &pending.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(WIDTH),
                    rows_per_image: None,
                },
            },
            pending.target.size(),
        );
        pending.state.store(ENCODED, Ordering::Release);
        Ok(())
    }
}
