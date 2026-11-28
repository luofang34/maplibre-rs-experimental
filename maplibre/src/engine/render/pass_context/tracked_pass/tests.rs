use super::{PassStats, TrackedRenderPass};

struct Gpu {
    device: wgpu::Device,
    /// Encoders need the device's queue alive.
    _queue: wgpu::Queue,
    target: wgpu::TextureView,
    pipelines: [wgpu::RenderPipeline; 2],
    groups: [wgpu::BindGroup; 2],
    dynamic_group: wgpu::BindGroup,
    buffer: wgpu::Buffer,
}

const SHADER: &str = "
@vertex fn vs(@location(0) position: vec2<f32>) -> @builtin(position) vec4<f32> {
    return vec4<f32>(position, 0.0, 1.0);
}
@fragment fn fs() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }
";

async fn gpu() -> Gpu {
    let instance = wgpu::Instance::default();
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .expect("GPU adapter");
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await
        .expect("GPU device");
    let target = device
        .create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default());
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[],
    });
    let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 512,
        usage: wgpu::BufferUsages::UNIFORM,
        mapped_at_creation: false,
    });
    let dynamic_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: true,
                min_binding_size: wgpu::BufferSize::new(64),
            },
            count: None,
        }],
    });
    let dynamic_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &dynamic_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &uniform_buffer,
                offset: 0,
                size: wgpu::BufferSize::new(64),
            }),
        }],
    });
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        bind_group_layouts: &[Some(&layout), Some(&dynamic_layout)],
        ..Default::default()
    });
    let pipeline = || {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 8,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::TextureFormat::Rgba8Unorm.into())],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        })
    };
    let group = || {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &[],
        })
    };
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 256,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::INDEX,
        mapped_at_creation: false,
    });
    Gpu {
        pipelines: [pipeline(), pipeline()],
        groups: [group(), group()],
        dynamic_group,
        device,
        _queue: queue,
        target,
        buffer,
    }
}

/// Records `record` into a fresh pass and returns what it counted.
fn recorded(gpu: &Gpu, record: impl FnOnce(&mut TrackedRenderPass<'_>)) -> PassStats {
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: None,
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &gpu.target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations::default(),
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    let mut pass = TrackedRenderPass::new(pass);
    record(&mut pass);
    pass.stats()
}

#[tokio::test]
async fn binding_what_is_bound_again_records_nothing() {
    let gpu = gpu().await;
    let stats = recorded(&gpu, |pass| {
        for _ in 0..3 {
            pass.set_pipeline(&gpu.pipelines[0]);
            pass.set_bind_group(0, &gpu.groups[0], &[]);
            pass.set_vertex_buffer(0, gpu.buffer.slice(0..64));
            pass.set_index_buffer(gpu.buffer.slice(64..128), wgpu::IndexFormat::Uint16);
            pass.set_stencil_reference(3);
            pass.draw_indexed(0..3, 0, 0..1);
        }
    });
    assert_eq!(
        stats,
        PassStats {
            draws: 3,
            state_changes: 5,
            redundant_state_changes: 10,
        }
    );
}

#[tokio::test]
async fn any_change_of_state_is_recorded() {
    let gpu = gpu().await;
    let stats = recorded(&gpu, |pass| {
        pass.set_pipeline(&gpu.pipelines[0]);
        pass.set_pipeline(&gpu.pipelines[1]);
        pass.set_bind_group(0, &gpu.groups[0], &[]);
        pass.set_bind_group(0, &gpu.groups[1], &[]);
        pass.set_vertex_buffer(0, gpu.buffer.slice(0..64));
        pass.set_vertex_buffer(0, gpu.buffer.slice(8..64));
        pass.set_vertex_buffer(0, gpu.buffer.slice(8..72));
        pass.set_vertex_buffer(1, gpu.buffer.slice(8..72));
        pass.set_index_buffer(gpu.buffer.slice(0..64), wgpu::IndexFormat::Uint16);
        pass.set_index_buffer(gpu.buffer.slice(0..64), wgpu::IndexFormat::Uint32);
        pass.set_stencil_reference(1);
        pass.set_stencil_reference(2);
        pass.draw(0..3, 0..1);
    });
    assert_eq!(
        stats,
        PassStats {
            draws: 1,
            state_changes: 12,
            redundant_state_changes: 0,
        }
    );
}

#[tokio::test]
async fn redundant_set_bind_group_zero_allocations_and_non_changing() {
    let gpu = gpu().await;
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: None,
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &gpu.target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations::default(),
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    let mut pass = TrackedRenderPass::new(pass);

    // Initial binding with empty offsets
    pass.set_bind_group(0, &gpu.groups[0], &[]);
    assert_eq!(pass.stats().state_changes, 1);
    assert_eq!(pass.stats().redundant_state_changes, 0);
    assert!(!pass.bound.bind_groups[0].as_ref().unwrap().1.spilled());

    // Redundant call with same bind group and empty offsets
    pass.set_bind_group(0, &gpu.groups[0], &[]);
    assert_eq!(pass.stats().state_changes, 1);
    assert_eq!(pass.stats().redundant_state_changes, 1);
    assert!(!pass.bound.bind_groups[0].as_ref().unwrap().1.spilled());

    // Binding dynamic group at index 1 with dynamic offset [0]
    pass.set_bind_group(1, &gpu.dynamic_group, &[0]);
    assert_eq!(pass.stats().state_changes, 2);
    assert_eq!(pass.stats().redundant_state_changes, 1);
    assert_eq!(
        pass.bound.bind_groups[1].as_ref().unwrap().1.as_slice(),
        &[0]
    );
    // SmallVec with capacity 2 has not spilled to the heap (zero allocations)
    assert!(!pass.bound.bind_groups[1].as_ref().unwrap().1.spilled());

    // Redundant call with same dynamic offset [0]
    pass.set_bind_group(1, &gpu.dynamic_group, &[0]);
    assert_eq!(pass.stats().state_changes, 2);
    assert_eq!(pass.stats().redundant_state_changes, 2);
    assert!(!pass.bound.bind_groups[1].as_ref().unwrap().1.spilled());

    // Changed dynamic offset to [256]
    pass.set_bind_group(1, &gpu.dynamic_group, &[256]);
    assert_eq!(pass.stats().state_changes, 3);
    assert_eq!(pass.stats().redundant_state_changes, 2);
    assert_eq!(
        pass.bound.bind_groups[1].as_ref().unwrap().1.as_slice(),
        &[256]
    );
    assert!(!pass.bound.bind_groups[1].as_ref().unwrap().1.spilled());

    // Redundant call with [256]
    pass.set_bind_group(1, &gpu.dynamic_group, &[256]);
    assert_eq!(pass.stats().state_changes, 3);
    assert_eq!(pass.stats().redundant_state_changes, 3);
    assert!(!pass.bound.bind_groups[1].as_ref().unwrap().1.spilled());
}

#[tokio::test]
async fn redundant_vertex_and_index_buffer_bindings_record_as_redundant() {
    let gpu = gpu().await;
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: None,
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &gpu.target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations::default(),
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    let mut pass = TrackedRenderPass::new(pass);

    // Initial vertex buffer binding
    pass.set_vertex_buffer(0, gpu.buffer.slice(0..64));
    assert_eq!(pass.stats().state_changes, 1);
    assert_eq!(pass.stats().redundant_state_changes, 0);

    // Redundant vertex buffer binding with exact same slice
    pass.set_vertex_buffer(0, gpu.buffer.slice(0..64));
    assert_eq!(pass.stats().state_changes, 1);
    assert_eq!(pass.stats().redundant_state_changes, 1);

    // Initial index buffer binding
    pass.set_index_buffer(gpu.buffer.slice(64..128), wgpu::IndexFormat::Uint16);
    assert_eq!(pass.stats().state_changes, 2);
    assert_eq!(pass.stats().redundant_state_changes, 1);

    // Redundant index buffer binding with exact same slice and format
    pass.set_index_buffer(gpu.buffer.slice(64..128), wgpu::IndexFormat::Uint16);
    assert_eq!(pass.stats().state_changes, 2);
    assert_eq!(pass.stats().redundant_state_changes, 2);

    // Different format triggers change
    pass.set_index_buffer(gpu.buffer.slice(64..128), wgpu::IndexFormat::Uint32);
    assert_eq!(pass.stats().state_changes, 3);
    assert_eq!(pass.stats().redundant_state_changes, 2);
}
