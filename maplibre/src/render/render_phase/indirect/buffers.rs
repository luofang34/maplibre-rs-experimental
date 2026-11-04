use std::{mem::size_of, num::NonZeroU32, ops::Range};

use super::{DrawIndexedIndirectArgs, DrawIndirectArgs, IndirectBatchSet};
use crate::render::{tracked_pass::DrawCapabilities, upload_queue::UploadQueue};

const MIN_INDIRECT_CAPACITY: usize = 64;

struct BackingIndirectBuffer {
    buffer: wgpu::Buffer,
    capacity_elements: usize,
    usages: wgpu::BufferUsages,
}

/// Shared CPU staging and GPU indirect parameter buffers across binned render phases.
///
/// Supports both CPU-assembled indirect draw buffers (uploaded through [`UploadQueue`] for
/// accurate frame byte accounting) and direct GPU compute culling access when `STORAGE` buffer
/// bindings are supported by the device.
#[derive(Default)]
pub struct IndirectParametersBuffers {
    indexed_data: Vec<DrawIndexedIndirectArgs>,
    non_indexed_data: Vec<DrawIndirectArgs>,
    batch_sets_data: Vec<IndirectBatchSet>,
    uploaded_indexed_len: usize,
    uploaded_non_indexed_len: usize,
    uploaded_batch_sets_len: usize,
    indexed_gpu: Option<BackingIndirectBuffer>,
    non_indexed_gpu: Option<BackingIndirectBuffer>,
    batch_sets_gpu: Option<BackingIndirectBuffer>,
    gpu_count_enabled: bool,
}

impl IndirectParametersBuffers {
    /// Creates an empty indirect parameter buffer set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Clears staged CPU indirect arguments while retaining allocated CPU and GPU capacities.
    pub fn clear(&mut self) {
        self.indexed_data.clear();
        self.non_indexed_data.clear();
        self.batch_sets_data.clear();
        self.uploaded_indexed_len = 0;
        self.uploaded_non_indexed_len = 0;
        self.uploaded_batch_sets_len = 0;
    }

    /// Enables or disables GPU count-buffer dispatch (`multi_draw_[indexed_]indirect_count`) when
    /// [`DrawCapabilities::multi_draw_indirect_count`] is also supported by the device.
    pub fn set_gpu_count_enabled(&mut self, enabled: bool) {
        self.gpu_count_enabled = enabled;
    }

    /// Whether GPU count-buffer dispatch is requested for multi-draw batch sets.
    #[inline]
    pub const fn gpu_count_enabled(&self) -> bool {
        self.gpu_count_enabled
    }

    /// Appends one indexed indirect command and returns its slot index.
    pub fn push_indexed(&mut self, args: DrawIndexedIndirectArgs) -> u32 {
        let index = u32::try_from(self.indexed_data.len()).unwrap_or(u32::MAX);
        self.indexed_data.push(args);
        index
    }

    /// Appends a contiguous sequence of indexed indirect commands and returns their slot range.
    pub fn extend_indexed(
        &mut self,
        iter: impl IntoIterator<Item = DrawIndexedIndirectArgs>,
    ) -> Range<u32> {
        let start = u32::try_from(self.indexed_data.len()).unwrap_or(u32::MAX);
        self.indexed_data.extend(iter);
        let end = u32::try_from(self.indexed_data.len()).unwrap_or(u32::MAX);
        start..end
    }

    /// Appends one non-indexed indirect command and returns its slot index.
    pub fn push_non_indexed(&mut self, args: DrawIndirectArgs) -> u32 {
        let index = u32::try_from(self.non_indexed_data.len()).unwrap_or(u32::MAX);
        self.non_indexed_data.push(args);
        index
    }

    /// Appends a contiguous sequence of non-indexed indirect commands and returns their slot range.
    pub fn extend_non_indexed(
        &mut self,
        iter: impl IntoIterator<Item = DrawIndirectArgs>,
    ) -> Range<u32> {
        let start = u32::try_from(self.non_indexed_data.len()).unwrap_or(u32::MAX);
        self.non_indexed_data.extend(iter);
        let end = u32::try_from(self.non_indexed_data.len()).unwrap_or(u32::MAX);
        start..end
    }

    /// Registers a multi-draw batch set and returns its 1-based [`NonZeroU32`] handle.
    pub fn push_batch_set(&mut self, count: u32, base: u32) -> NonZeroU32 {
        self.batch_sets_data.push(IndirectBatchSet {
            indirect_parameters_count: count,
            indirect_parameters_base: base,
        });
        let one_based = u32::try_from(self.batch_sets_data.len()).unwrap_or(u32::MAX);
        NonZeroU32::new(one_based).unwrap_or(NonZeroU32::MIN)
    }

    /// Staged CPU indexed indirect commands for the current pass.
    #[inline]
    pub fn indexed_data(&self) -> &[DrawIndexedIndirectArgs] {
        &self.indexed_data
    }

    /// Staged CPU non-indexed indirect commands for the current pass.
    #[inline]
    pub fn non_indexed_data(&self) -> &[DrawIndirectArgs] {
        &self.non_indexed_data
    }

    /// Staged CPU batch-set count/base headers for the current pass.
    #[inline]
    pub fn batch_sets_data(&self) -> &[IndirectBatchSet] {
        &self.batch_sets_data
    }

    /// Borrows the GPU indexed indirect parameter buffer, if allocated.
    #[inline]
    pub fn indexed_buffer(&self) -> Option<&wgpu::Buffer> {
        self.indexed_gpu.as_ref().map(|b| &b.buffer)
    }

    /// Borrows the GPU non-indexed indirect parameter buffer, if allocated.
    #[inline]
    pub fn non_indexed_buffer(&self) -> Option<&wgpu::Buffer> {
        self.non_indexed_gpu.as_ref().map(|b| &b.buffer)
    }

    /// Borrows the GPU batch-set count/header buffer, if allocated.
    #[inline]
    pub fn batch_sets_buffer(&self) -> Option<&wgpu::Buffer> {
        self.batch_sets_gpu.as_ref().map(|b| &b.buffer)
    }

    /// Byte offset of `slot` within [`Self::indexed_buffer`].
    #[inline]
    pub const fn indexed_byte_offset(slot: u32) -> wgpu::BufferAddress {
        (slot as wgpu::BufferAddress) * DrawIndexedIndirectArgs::STRIDE
    }

    /// Byte offset of `slot` within [`Self::non_indexed_buffer`].
    #[inline]
    pub const fn non_indexed_byte_offset(slot: u32) -> wgpu::BufferAddress {
        (slot as wgpu::BufferAddress) * DrawIndirectArgs::STRIDE
    }

    /// Byte offset of `batch_set_index`'s `indirect_parameters_count` field in [`Self::batch_sets_buffer`].
    #[inline]
    pub const fn batch_set_count_byte_offset(batch_set_index: NonZeroU32) -> wgpu::BufferAddress {
        ((batch_set_index.get() - 1) as wgpu::BufferAddress) * IndirectBatchSet::STRIDE
    }

    fn buffer_usages(device: &wgpu::Device, capabilities: DrawCapabilities) -> wgpu::BufferUsages {
        let mut usages = wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST;
        if capabilities.vertex_storage_buffers
            && device.limits().max_storage_buffers_per_shader_stage > 0
        {
            usages |= wgpu::BufferUsages::STORAGE;
        }
        usages
    }

    /// Ensures the indexed indirect GPU buffer can hold at least `min_elements` entries for direct
    /// GPU compute shader binding.
    pub fn reserve_indexed_gpu(
        &mut self,
        device: &wgpu::Device,
        min_elements: usize,
        capabilities: DrawCapabilities,
    ) -> &wgpu::Buffer {
        let usages = Self::buffer_usages(device, capabilities);
        let (backing, _) = ensure_indirect_buffer::<DrawIndexedIndirectArgs>(
            device,
            &mut self.indexed_gpu,
            min_elements.max(1),
            usages,
            "binned_indexed_indirect_buffer",
        );
        &backing.buffer
    }

    /// Ensures the non-indexed indirect GPU buffer can hold at least `min_elements` entries for
    /// direct GPU compute shader binding.
    pub fn reserve_non_indexed_gpu(
        &mut self,
        device: &wgpu::Device,
        min_elements: usize,
        capabilities: DrawCapabilities,
    ) -> &wgpu::Buffer {
        let usages = Self::buffer_usages(device, capabilities);
        let (backing, _) = ensure_indirect_buffer::<DrawIndirectArgs>(
            device,
            &mut self.non_indexed_gpu,
            min_elements.max(1),
            usages,
            "binned_non_indexed_indirect_buffer",
        );
        &backing.buffer
    }

    /// Uploads any newly staged indirect arguments and batch-set headers through `queue`.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &UploadQueue,
        capabilities: DrawCapabilities,
    ) {
        if !capabilities.indirect_execution {
            return;
        }
        let usages = Self::buffer_usages(device, capabilities);
        upload_delta(
            device,
            queue,
            &self.indexed_data,
            &mut self.uploaded_indexed_len,
            &mut self.indexed_gpu,
            usages,
            "binned_indexed_indirect_buffer",
        );
        upload_delta(
            device,
            queue,
            &self.non_indexed_data,
            &mut self.uploaded_non_indexed_len,
            &mut self.non_indexed_gpu,
            usages,
            "binned_non_indexed_indirect_buffer",
        );
        upload_delta(
            device,
            queue,
            &self.batch_sets_data,
            &mut self.uploaded_batch_sets_len,
            &mut self.batch_sets_gpu,
            usages,
            "binned_indirect_batch_sets_buffer",
        );
    }
}

fn ensure_indirect_buffer<'a, T: bytemuck::Pod>(
    device: &wgpu::Device,
    slot: &'a mut Option<BackingIndirectBuffer>,
    required_elements: usize,
    usages: wgpu::BufferUsages,
    label: &'static str,
) -> (&'a BackingIndirectBuffer, bool) {
    let needs_realloc = slot.as_ref().is_none_or(|existing| {
        existing.capacity_elements < required_elements || existing.usages != usages
    });
    if needs_realloc {
        let capacity_elements = required_elements
            .max(MIN_INDIRECT_CAPACITY)
            .checked_next_power_of_two()
            .unwrap_or(required_elements);
        let size = (capacity_elements * size_of::<T>()) as wgpu::BufferAddress;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: usages,
            mapped_at_creation: false,
        });
        *slot = Some(BackingIndirectBuffer {
            buffer,
            capacity_elements,
            usages,
        });
    }
    let backing = slot.get_or_insert_with(|| {
        let capacity_elements = MIN_INDIRECT_CAPACITY;
        let size = (capacity_elements * size_of::<T>()) as wgpu::BufferAddress;
        BackingIndirectBuffer {
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: usages,
                mapped_at_creation: false,
            }),
            capacity_elements,
            usages,
        }
    });
    (backing, needs_realloc)
}

fn upload_delta<T: bytemuck::Pod>(
    device: &wgpu::Device,
    queue: &UploadQueue,
    data: &[T],
    uploaded_len: &mut usize,
    slot: &mut Option<BackingIndirectBuffer>,
    usages: wgpu::BufferUsages,
    label: &'static str,
) {
    if data.len() <= *uploaded_len {
        return;
    }
    let (backing, reallocated) =
        ensure_indirect_buffer::<T>(device, slot, data.len(), usages, label);
    let start = if reallocated { 0 } else { *uploaded_len };
    let byte_offset = (start * size_of::<T>()) as wgpu::BufferAddress;
    let slice_bytes = bytemuck::cast_slice(&data[start..]);
    queue.write_buffer(&backing.buffer, byte_offset, slice_bytes);
    *uploaded_len = data.len();
}
