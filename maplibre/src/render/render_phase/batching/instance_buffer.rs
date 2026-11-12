use std::{marker::PhantomData, mem::size_of, ops::Range};

use super::FallbackInstanceBufferMode;
use crate::render::{tracked_pass::DrawCapabilities, upload_queue::UploadQueue};

const MIN_INSTANCE_BUFFER_BYTES: usize = 1024;
const MIN_UBO_BINDING_ALIGN: usize = 16;

/// Contiguous CPU/GPU instance buffer for `BD`, supporting storage buffers, vertex-step
/// instance buffers, and aligned dynamic uniform fallback buffers.
///
/// Mirrors Bevy's `BatchedInstanceBuffer<BD>`. Packed storage/vertex backing (`gpu_buffer`) and
/// dynamic-uniform backing (`uniform_gpu_buffer`) are maintained independently so multiple phases
/// sharing `BD` with different [`FallbackInstanceBufferMode`]s never clobber each other.
pub struct BatchedInstanceBuffer<BD: bytemuck::Pod> {
    instances: Vec<BD>,
    packed_end_len: usize,
    aligned_uniform_bytes: Vec<u8>,
    padded_upload_scratch: Vec<u8>,
    uploaded_bytes: usize,
    uploaded_uniform_bytes: usize,
    aligned_stride: usize,
    fallback_mode: FallbackInstanceBufferMode,
    gpu_buffer: Option<wgpu::Buffer>,
    gpu_capacity_bytes: usize,
    gpu_usages: wgpu::BufferUsages,
    uniform_gpu_buffer: Option<wgpu::Buffer>,
    uniform_gpu_capacity_bytes: usize,
    retired_buffers: Vec<wgpu::Buffer>,
    _marker: PhantomData<BD>,
}

impl<BD: bytemuck::Pod> Drop for BatchedInstanceBuffer<BD> {
    fn drop(&mut self) {
        for buffer in self.retired_buffers.drain(..) {
            buffer.destroy();
        }
        if let Some(buffer) = self.gpu_buffer.take() {
            buffer.destroy();
        }
        if let Some(buffer) = self.uniform_gpu_buffer.take() {
            buffer.destroy();
        }
    }
}

impl<BD: bytemuck::Pod> Default for BatchedInstanceBuffer<BD> {
    fn default() -> Self {
        Self::new()
    }
}

impl<BD: bytemuck::Pod> BatchedInstanceBuffer<BD> {
    /// Creates an empty [`BatchedInstanceBuffer`] defaulting to [`FallbackInstanceBufferMode::DynamicUniform`].
    pub fn new() -> Self {
        Self {
            instances: Vec::new(),
            packed_end_len: 0,
            aligned_uniform_bytes: Vec::new(),
            padded_upload_scratch: Vec::new(),
            uploaded_bytes: 0,
            uploaded_uniform_bytes: 0,
            aligned_stride: align_up(size_of::<BD>().max(MIN_UBO_BINDING_ALIGN), 256),
            fallback_mode: FallbackInstanceBufferMode::DynamicUniform,
            gpu_buffer: None,
            gpu_capacity_bytes: 0,
            gpu_usages: wgpu::BufferUsages::empty(),
            uniform_gpu_buffer: None,
            uniform_gpu_capacity_bytes: 0,
            retired_buffers: Vec::new(),
            _marker: PhantomData,
        }
    }

    /// Configures the default fallback buffer mode used when a phase does not specify its own.
    pub fn with_fallback_mode(mut self, mode: FallbackInstanceBufferMode) -> Self {
        self.fallback_mode = mode;
        self
    }

    /// Updates the default fallback buffer mode used when a phase does not specify its own.
    pub fn set_fallback_mode(&mut self, mode: FallbackInstanceBufferMode) {
        self.fallback_mode = mode;
    }

    /// Default fallback buffer mode used when `vertex_storage_buffers` is `false` and a phase has
    /// not configured an explicit override.
    #[inline]
    pub const fn fallback_mode(&self) -> FallbackInstanceBufferMode {
        self.fallback_mode
    }

    /// Clears CPU-staged instances for a new pass while retaining CPU and GPU capacities.
    pub fn clear(&mut self) {
        for buffer in self.retired_buffers.drain(..) {
            buffer.destroy();
        }
        self.instances.clear();
        self.packed_end_len = 0;
        self.aligned_uniform_bytes.clear();
        self.uploaded_bytes = 0;
        self.uploaded_uniform_bytes = 0;
    }

    /// Number of instances staged in the current pass.
    #[inline]
    pub fn len(&self) -> usize {
        self.instances.len()
    }

    /// Whether zero instances are staged in the current pass.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    /// Tightly packed CPU slice of staged instances in deterministic draw order.
    #[inline]
    pub fn instances(&self) -> &[BD] {
        &self.instances
    }

    /// Aligned byte buffer staged when operating in [`FallbackInstanceBufferMode::DynamicUniform`].
    #[inline]
    pub fn aligned_uniform_bytes(&self) -> &[u8] {
        &self.aligned_uniform_bytes
    }

    /// Byte stride per element in [`Self::aligned_uniform_bytes`] when using dynamic uniforms.
    #[inline]
    pub const fn aligned_uniform_stride(&self) -> usize {
        self.aligned_stride
    }

    /// Minimum uniform binding size (`>= 16` bytes, multiple of 16) for one `BD` element.
    #[inline]
    pub const fn uniform_binding_size() -> Option<wgpu::BufferSize> {
        let raw = size_of::<BD>();
        if raw == 0 {
            return None;
        }
        let aligned = align_up(raw, MIN_UBO_BINDING_ALIGN);
        wgpu::BufferSize::new(aligned as u64)
    }

    /// Borrows the packed storage/vertex GPU buffer, falling back to [`Self::uniform_buffer`]
    /// when only dynamic-uniform staging is active.
    #[inline]
    pub fn buffer(&self) -> Option<&wgpu::Buffer> {
        self.gpu_buffer
            .as_ref()
            .or(self.uniform_gpu_buffer.as_ref())
    }

    /// Borrows the dynamic-uniform GPU buffer (`UNIFORM | COPY_DST`) after [`Self::upload`] has
    /// uploaded dynamic-uniform instances.
    #[inline]
    pub fn uniform_buffer(&self) -> Option<&wgpu::Buffer> {
        self.uniform_gpu_buffer.as_ref()
    }

    /// Current packed storage/vertex GPU buffer capacity in bytes (or dynamic-uniform capacity
    /// when only dynamic-uniform staging has been allocated).
    #[inline]
    pub const fn gpu_capacity_bytes(&self) -> usize {
        if self.gpu_capacity_bytes > 0 {
            self.gpu_capacity_bytes
        } else {
            self.uniform_gpu_capacity_bytes
        }
    }

    /// Current dynamic-uniform GPU buffer capacity in bytes (`0` before first dynamic-uniform upload).
    #[inline]
    pub const fn uniform_gpu_capacity_bytes(&self) -> usize {
        self.uniform_gpu_capacity_bytes
    }

    /// Returns the sub-slice covering `instance_range` in the packed storage/vertex GPU buffer for
    /// `VertexStepMode::Instance` binding.
    ///
    /// Particularly useful on WebGL2 (`indirect_first_instance == false`), allowing a pass to
    /// bind only `instance_range` at vertex slot `N` while [`draw_binned_mesh`](crate::render::render_phase::draw_binned_mesh)
    /// dispatches `0..count` with `first_instance == 0`.
    pub fn instance_slice(&self, instance_range: Range<u32>) -> Option<wgpu::BufferSlice<'_>> {
        let buffer = self.gpu_buffer.as_ref()?;
        let elem_size = size_of::<BD>() as wgpu::BufferAddress;
        if elem_size == 0 {
            return None;
        }
        let start = (instance_range.start as wgpu::BufferAddress) * elem_size;
        let end = (instance_range.end as wgpu::BufferAddress) * elem_size;
        Some(buffer.slice(start..end))
    }

    /// Appends one tightly packed instance for storage or `VertexStepMode::Instance` consumption
    /// and returns its instance index.
    pub fn push(&mut self, value: BD) -> u32 {
        let index = u32::try_from(self.instances.len()).unwrap_or(u32::MAX);
        self.instances.push(value);
        self.packed_end_len = self.instances.len();
        index
    }

    /// Appends one instance aligned to `min_uniform_alignment` for dynamic uniform binding,
    /// returning `(instance_index, dynamic_byte_offset)`.
    pub fn push_dynamic_uniform(&mut self, value: BD, min_uniform_alignment: u32) -> (u32, u32) {
        let align = (min_uniform_alignment as usize).max(MIN_UBO_BINDING_ALIGN);
        let stride = align_up(size_of::<BD>().max(MIN_UBO_BINDING_ALIGN), align);
        self.aligned_stride = stride;
        let index = u32::try_from(self.instances.len()).unwrap_or(u32::MAX);
        let byte_offset = self.aligned_uniform_bytes.len();
        let offset_u32 = u32::try_from(byte_offset).unwrap_or(u32::MAX);
        let bytes = bytemuck::bytes_of(&value);
        self.aligned_uniform_bytes.resize(byte_offset + stride, 0u8);
        self.aligned_uniform_bytes[byte_offset..byte_offset + bytes.len()].copy_from_slice(bytes);
        self.instances.push(value);
        (index, offset_u32)
    }

    /// Uploads newly staged packed and/or dynamic-uniform instance slices to the GPU via
    /// [`UploadQueue::write_buffer`], growing each backing `wgpu::Buffer` by 2x as needed.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &UploadQueue,
        capabilities: DrawCapabilities,
    ) {
        if self.instances.is_empty() || size_of::<BD>() == 0 {
            return;
        }
        if capabilities.vertex_storage_buffers {
            let raw_bytes: &[u8] = bytemuck::cast_slice(&self.instances);
            let usages = wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::VERTEX
                | wgpu::BufferUsages::COPY_DST;
            upload_delta_bytes(
                device,
                queue,
                "batched_instance_buffer",
                usages,
                raw_bytes,
                &mut self.gpu_buffer,
                &mut self.gpu_capacity_bytes,
                Some(&mut self.gpu_usages),
                &mut self.uploaded_bytes,
                &mut self.padded_upload_scratch,
                &mut self.retired_buffers,
            );
            return;
        }

        if self.packed_end_len > 0 {
            let raw_bytes: &[u8] = bytemuck::cast_slice(&self.instances[..self.packed_end_len]);
            let usages = wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST;
            upload_delta_bytes(
                device,
                queue,
                "batched_instance_vertex_buffer",
                usages,
                raw_bytes,
                &mut self.gpu_buffer,
                &mut self.gpu_capacity_bytes,
                Some(&mut self.gpu_usages),
                &mut self.uploaded_bytes,
                &mut self.padded_upload_scratch,
                &mut self.retired_buffers,
            );
        }

        if !self.aligned_uniform_bytes.is_empty() {
            let usages = wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST;
            upload_delta_bytes(
                device,
                queue,
                "batched_instance_uniform_buffer",
                usages,
                &self.aligned_uniform_bytes,
                &mut self.uniform_gpu_buffer,
                &mut self.uniform_gpu_capacity_bytes,
                None,
                &mut self.uploaded_uniform_bytes,
                &mut self.padded_upload_scratch,
                &mut self.retired_buffers,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn upload_delta_bytes(
    device: &wgpu::Device,
    queue: &UploadQueue,
    label: &'static str,
    usages: wgpu::BufferUsages,
    raw_bytes: &[u8],
    gpu_buffer: &mut Option<wgpu::Buffer>,
    gpu_capacity_bytes: &mut usize,
    tracked_usages: Option<&mut wgpu::BufferUsages>,
    uploaded_bytes: &mut usize,
    padded_upload_scratch: &mut Vec<u8>,
    retired_buffers: &mut Vec<wgpu::Buffer>,
) {
    if raw_bytes.len() <= *uploaded_bytes {
        return;
    }
    let copy_align = wgpu::COPY_BUFFER_ALIGNMENT as usize;
    let total_padded_bytes = align_up(raw_bytes.len(), copy_align);
    let usage_mismatch = tracked_usages
        .as_deref()
        .is_some_and(|prev| *prev != usages);
    let reallocated =
        gpu_buffer.is_none() || *gpu_capacity_bytes < total_padded_bytes || usage_mismatch;
    if reallocated {
        let mut new_cap = (*gpu_capacity_bytes).max(MIN_INSTANCE_BUFFER_BYTES);
        while new_cap < total_padded_bytes {
            new_cap = new_cap.saturating_mul(2).max(total_padded_bytes);
        }
        if let Some(old) = gpu_buffer.replace(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: new_cap as wgpu::BufferAddress,
            usage: usages,
            mapped_at_creation: false,
        })) {
            retired_buffers.push(old);
        }
        *gpu_capacity_bytes = new_cap;
        if let Some(prev) = tracked_usages {
            *prev = usages;
        }
    }

    let start_byte = if reallocated {
        0
    } else {
        (*uploaded_bytes / copy_align) * copy_align
    };
    let delta_raw = &raw_bytes[start_byte..];
    let write_bytes: &[u8] = if delta_raw.len().is_multiple_of(copy_align) {
        delta_raw
    } else {
        let padded_len = align_up(delta_raw.len(), copy_align);
        padded_upload_scratch.clear();
        padded_upload_scratch.resize(padded_len, 0u8);
        padded_upload_scratch[..delta_raw.len()].copy_from_slice(delta_raw);
        padded_upload_scratch.as_slice()
    };

    if let Some(buffer) = gpu_buffer.as_ref() {
        queue.write_buffer(buffer, start_byte as wgpu::BufferAddress, write_bytes);
    }
    *uploaded_bytes = raw_bytes.len();
}

#[inline]
pub(super) const fn align_up(value: usize, alignment: usize) -> usize {
    if alignment <= 1 {
        return value;
    }
    let rem = value % alignment;
    if rem == 0 {
        value
    } else {
        value + (alignment - rem)
    }
}
