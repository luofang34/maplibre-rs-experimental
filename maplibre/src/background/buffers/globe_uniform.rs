//! Dynamic uniform buffer pool supplying [`GlobeBackgroundUniform`] at `@group(1) @binding(0)`.

use std::num::NonZeroU64;

use super::{AllocatedBuffers, BackgroundGpuBuffers};
use crate::render::{
    shaders::{BackgroundLayerMetadata, GlobeBackgroundUniform},
    upload_queue::UploadQueue,
};

const UNIFORM_SIZE: usize = std::mem::size_of::<GlobeBackgroundUniform>();

/// Creates the bind group layout for globe background layer uniforms (`@group(1) @binding(0)`).
pub(crate) fn globe_layer_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Globe Background Layer Bind Group Layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: true,
                min_binding_size: NonZeroU64::new(UNIFORM_SIZE as u64),
            },
            count: None,
        }],
    })
}

fn create_uniform_buffer(device: &wgpu::Device, stride: u64, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Globe Background Layer Uniform Buffer"),
        size: (capacity.max(1) as u64) * stride,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn create_uniform_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Globe Background Layer Bind Group"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer,
                offset: 0,
                size: NonZeroU64::new(UNIFORM_SIZE as u64),
            }),
        }],
    })
}

/// Persistent GPU dynamic-uniform ring holding one aligned [`GlobeBackgroundUniform`] per background layer.
pub(in crate::background) struct GlobeLayerUniformPool {
    pub(super) layout: wgpu::BindGroupLayout,
    buffer: wgpu::Buffer,
    pub(super) bind_group: wgpu::BindGroup,
    pub(super) stride: u64,
    capacity: usize,
    staged: Vec<u8>,
    last_bytes: Vec<u8>,
}

impl Drop for GlobeLayerUniformPool {
    fn drop(&mut self) {
        self.buffer.destroy();
    }
}

impl GlobeLayerUniformPool {
    pub(super) fn new(device: &wgpu::Device, capacity: usize) -> Self {
        let layout = globe_layer_bind_group_layout(device);
        let align = u64::from(device.limits().min_uniform_buffer_offset_alignment);
        let stride = align.max(UNIFORM_SIZE as u64);
        let capacity = capacity.max(1);
        let buffer = create_uniform_buffer(device, stride, capacity);
        let bind_group = create_uniform_bind_group(device, &layout, &buffer);
        Self {
            layout,
            buffer,
            bind_group,
            stride,
            capacity,
            staged: Vec::new(),
            last_bytes: Vec::new(),
        }
    }

    /// Writes aligned [`GlobeBackgroundUniform`] slices, returning `true` if reallocated.
    pub(super) fn write_layers(
        &mut self,
        device: &wgpu::Device,
        queue: &UploadQueue,
        layers: &[BackgroundLayerMetadata],
    ) -> bool {
        if layers.is_empty() {
            return false;
        }
        let needed = layers.len();
        let reallocated = needed > self.capacity;
        if reallocated {
            let new_capacity = (self.capacity * 2).max(needed);
            let new_buffer = create_uniform_buffer(device, self.stride, new_capacity);
            let new_bind_group = create_uniform_bind_group(device, &self.layout, &new_buffer);
            let old_buffer = std::mem::replace(&mut self.buffer, new_buffer);
            queue.cancel_staged_buffer(&old_buffer);
            old_buffer.destroy();
            self.bind_group = new_bind_group;
            self.capacity = new_capacity;
            self.last_bytes.clear();
        }
        let stride = self.stride as usize;
        self.staged.clear();
        self.staged.resize(needed * stride, 0);
        for (i, meta) in layers.iter().enumerate() {
            let uniform = GlobeBackgroundUniform::from_layer(meta);
            let start = i * stride;
            self.staged[start..start + UNIFORM_SIZE].copy_from_slice(bytemuck::bytes_of(&uniform));
        }
        if self.last_bytes != self.staged {
            queue.write_buffer(&self.buffer, 0, &self.staged);
            self.last_bytes.clear();
            self.last_bytes.extend_from_slice(&self.staged);
        }
        reallocated
    }
}

impl BackgroundGpuBuffers {
    /// Returns the globe background layer bind group layout (`@group(1) @binding(0)`).
    pub(crate) fn globe_layer_layout(&mut self, device: &wgpu::Device) -> wgpu::BindGroupLayout {
        self.ensure_allocated(device).globe_uniform.layout.clone()
    }

    /// Returns the persistent globe layer bind group and byte stride when allocated.
    pub(crate) fn globe_layer_binding(&self) -> Option<(&wgpu::BindGroup, u32)> {
        self.inner.as_ref().map(|inner| {
            (
                &inner.globe_uniform.bind_group,
                inner.globe_uniform.stride as u32,
            )
        })
    }

    /// Uploads staged layer uniforms and returns the current bind group and dynamic-offset stride.
    pub(crate) fn write_staged_globe_layers(
        &mut self,
        device: &wgpu::Device,
        queue: &UploadQueue,
    ) -> (wgpu::BindGroup, u32) {
        let Self { inner, scratch } = self;
        let inner = inner.get_or_insert_with(|| AllocatedBuffers::new(device));
        if inner
            .globe_uniform
            .write_layers(device, queue, &scratch.metadatas)
        {
            inner.bump_generation();
        }
        (
            inner.globe_uniform.bind_group.clone(),
            inner.globe_uniform.stride as u32,
        )
    }
}
