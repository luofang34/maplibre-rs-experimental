use std::{marker::PhantomData, mem::size_of, ops::Range};

use super::FallbackInstanceBufferMode;
use crate::render::{tracked_pass::DrawCapabilities, upload_queue::UploadQueue};

const MIN_INSTANCE_BUFFER_BYTES: usize = 1024;
const MIN_UBO_BINDING_ALIGN: usize = 16;

/// Contiguous CPU/GPU instance buffer for `BD`, supporting storage buffers, vertex-step
/// instance buffers, and aligned dynamic uniform fallback buffers.
///
/// Mirrors Bevy's `BatchedInstanceBuffer<BD>`.
pub struct BatchedInstanceBuffer<BD: bytemuck::Pod> {
    instances: Vec<BD>,
    aligned_uniform_bytes: Vec<u8>,
    padded_upload_scratch: Vec<u8>,
    uploaded_bytes: usize,
    aligned_stride: usize,
    fallback_mode: FallbackInstanceBufferMode,
    gpu_buffer: Option<wgpu::Buffer>,
    gpu_capacity_bytes: usize,
    gpu_usages: wgpu::BufferUsages,
    _marker: PhantomData<BD>,
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
            aligned_uniform_bytes: Vec::new(),
            padded_upload_scratch: Vec::new(),
            uploaded_bytes: 0,
            aligned_stride: align_up(size_of::<BD>().max(MIN_UBO_BINDING_ALIGN), 256),
            fallback_mode: FallbackInstanceBufferMode::DynamicUniform,
            gpu_buffer: None,
            gpu_capacity_bytes: 0,
            gpu_usages: wgpu::BufferUsages::empty(),
            _marker: PhantomData,
        }
    }

    /// Configures the fallback buffer mode used when `vertex_storage_buffers` is `false`.
    pub fn with_fallback_mode(mut self, mode: FallbackInstanceBufferMode) -> Self {
        self.fallback_mode = mode;
        self
    }

    /// Updates the fallback buffer mode used when `vertex_storage_buffers` is `false`.
    pub fn set_fallback_mode(&mut self, mode: FallbackInstanceBufferMode) {
        self.fallback_mode = mode;
    }

    /// Fallback buffer mode used when `vertex_storage_buffers` is `false`.
    #[inline]
    pub const fn fallback_mode(&self) -> FallbackInstanceBufferMode {
        self.fallback_mode
    }

    /// Clears CPU-staged instances for a new pass while retaining CPU and GPU capacities.
    pub fn clear(&mut self) {
        self.instances.clear();
        self.aligned_uniform_bytes.clear();
        self.uploaded_bytes = 0;
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

    /// Borrows the underlying GPU buffer after [`Self::upload`] has run.
    #[inline]
    pub fn buffer(&self) -> Option<&wgpu::Buffer> {
        self.gpu_buffer.as_ref()
    }

    /// Current GPU buffer capacity in bytes (`0` before first upload).
    #[inline]
    pub const fn gpu_capacity_bytes(&self) -> usize {
        self.gpu_capacity_bytes
    }

    /// Returns the sub-slice covering `instance_range` for `VertexStepMode::Instance` binding.
    ///
    /// Particularly useful on WebGL2 (`indirect_first_instance == false`), allowing a pass to
    /// bind only `instance_range` at vertex slot `N` and issue `draw_indexed(..., 0..count)`
    /// with `first_instance == 0`.
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

    /// Appends one tightly packed instance and returns its instance index.
    pub fn push(&mut self, value: BD) -> u32 {
        let index =
            u32::try_from(self.instances.len()).expect("BatchedInstanceBuffer exceeded u32::MAX");
        self.instances.push(value);
        index
    }

    /// Appends one instance aligned to `min_uniform_alignment` for dynamic uniform binding,
    /// returning `(instance_index, dynamic_byte_offset)`.
    pub fn push_dynamic_uniform(&mut self, value: BD, min_uniform_alignment: u32) -> (u32, u32) {
        let align = (min_uniform_alignment as usize).max(MIN_UBO_BINDING_ALIGN);
        let stride = align_up(size_of::<BD>().max(MIN_UBO_BINDING_ALIGN), align);
        self.aligned_stride = stride;
        let index = self.push(value);
        let byte_offset = self.aligned_uniform_bytes.len();
        let offset_u32 = u32::try_from(byte_offset)
            .expect("dynamic uniform buffer byte offset exceeded u32::MAX");
        let bytes = bytemuck::bytes_of(self.instances.last().expect("just pushed"));
        self.aligned_uniform_bytes.resize(byte_offset + stride, 0u8);
        self.aligned_uniform_bytes[byte_offset..byte_offset + bytes.len()].copy_from_slice(bytes);
        (index, offset_u32)
    }

    /// Uploads staged instance data to the GPU via [`UploadQueue::write_buffer`], growing the
    /// backing `wgpu::Buffer` by 2x whenever capacity is exceeded.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &UploadQueue,
        capabilities: DrawCapabilities,
    ) {
        if self.instances.is_empty() || size_of::<BD>() == 0 {
            return;
        }
        let use_dynamic_uniform = !capabilities.vertex_storage_buffers
            && self.fallback_mode == FallbackInstanceBufferMode::DynamicUniform;

        let usages = if capabilities.vertex_storage_buffers {
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST
        } else if use_dynamic_uniform {
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST
        } else {
            wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST
        };

        let raw_bytes: &[u8] = if use_dynamic_uniform {
            &self.aligned_uniform_bytes
        } else {
            bytemuck::cast_slice(&self.instances)
        };
        if raw_bytes.len() <= self.uploaded_bytes {
            return;
        }
        let copy_align = wgpu::COPY_BUFFER_ALIGNMENT as usize;
        let total_padded_bytes = align_up(raw_bytes.len(), copy_align);
        let reallocated = self.gpu_buffer.is_none()
            || self.gpu_capacity_bytes < total_padded_bytes
            || self.gpu_usages != usages;
        if reallocated {
            let mut new_cap = self.gpu_capacity_bytes.max(MIN_INSTANCE_BUFFER_BYTES);
            while new_cap < total_padded_bytes {
                new_cap = new_cap.saturating_mul(2).max(total_padded_bytes);
            }
            self.gpu_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("batched_instance_buffer"),
                size: new_cap as wgpu::BufferAddress,
                usage: usages,
                mapped_at_creation: false,
            }));
            self.gpu_capacity_bytes = new_cap;
            self.gpu_usages = usages;
        }

        let start_byte = if reallocated {
            0
        } else {
            (self.uploaded_bytes / copy_align) * copy_align
        };
        let delta_raw = &raw_bytes[start_byte..];
        let write_bytes: &[u8] = if delta_raw.len().is_multiple_of(copy_align) {
            delta_raw
        } else {
            let padded_len = align_up(delta_raw.len(), copy_align);
            self.padded_upload_scratch.clear();
            self.padded_upload_scratch.resize(padded_len, 0u8);
            self.padded_upload_scratch[..delta_raw.len()].copy_from_slice(delta_raw);
            &self.padded_upload_scratch
        };

        if let Some(buffer) = &self.gpu_buffer {
            queue.write_buffer(buffer, start_byte as wgpu::BufferAddress, write_bytes);
        }
        self.uploaded_bytes = raw_bytes.len();
    }
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
