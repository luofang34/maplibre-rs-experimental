//! The renderer's queue, counting the bytes each frame writes to buffers and textures, and
//! coalescing small aligned buffer writes into one per-frame staging upload.
//!
//! Bytes reach the GPU through [`UploadQueue::write_buffer`], [`UploadQueue::write_texture`],
//! [`UploadQueue::create_buffer_init`] and [`UploadQueue::create_texture_with_data`]. The queue
//! is not reachable as a [`wgpu::Queue`] except through [`UploadQueue::inner`], and the
//! crate denies the uncounted wgpu methods that its clippy.toml lists, so clippy rejects an
//! upload that skips the count.

use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Mutex,
};

use wgpu::util::DeviceExt;

#[cfg(not(target_arch = "wasm32"))]
mod native_ring;
#[cfg(target_arch = "wasm32")]
mod wasm_belt;

/// Maximum single `write_buffer` payload (exclusive) staged into the per-frame coalescing belt.
#[cfg(target_arch = "wasm32")]
const MAX_STAGED_WRITE_BYTES: usize = 65_536;
/// Maximum single `write_buffer` payload (exclusive) staged into the per-frame coalescing belt.
#[cfg(not(target_arch = "wasm32"))]
const MAX_STAGED_WRITE_BYTES: usize = 256 * 1024;

/// Steady-state post-burst staging buffer ceiling (`64 KiB`).
const SHRINK_STAGING_BUFFER_CAPACITY: u64 = 64 * 1024;

/// Hard upper bound on the persistent coalescing staging buffer (`1 MiB` on native, `2 MiB` on
/// wasm32) so initial tile upload bursts never pin tens of megabytes of transient staging memory.
#[cfg(target_arch = "wasm32")]
const MAX_STAGING_BUFFER_CAPACITY: u64 = 2 * 1024 * 1024;
/// Hard upper bound on the persistent coalescing staging buffer (`1 MiB` on native).
#[cfg(not(target_arch = "wasm32"))]
const MAX_STAGING_BUFFER_CAPACITY: u64 = 1024 * 1024;

#[derive(Debug)]
struct StagedCopy {
    dst: wgpu::Buffer,
    src_offset: u64,
    dst_offset: u64,
    size: u64,
}

#[derive(Debug, Default)]
struct StagingBeltState {
    payload: Vec<u8>,
    copies: Vec<StagedCopy>,
    #[cfg(not(target_arch = "wasm32"))]
    cached_max_buf: Option<u64>,
    #[cfg(target_arch = "wasm32")]
    wasm_belt: wasm_belt::WasmStagingBelt,
    #[cfg(not(target_arch = "wasm32"))]
    native_ring: native_ring::NativeStagingRing,
}

fn flush_copies_direct(queue: &wgpu::Queue, payload: &mut Vec<u8>, copies: &mut Vec<StagedCopy>) {
    for copy in copies.drain(..) {
        let start = copy.src_offset as usize;
        let end = start + copy.size as usize;
        if let Some(slice) = payload.get(start..end) {
            #[expect(
                clippy::disallowed_methods,
                reason = "UploadQueue flushes staged writes here after counting bytes in write_buffer"
            )]
            queue.write_buffer(&copy.dst, copy.dst_offset, slice);
        }
    }
    payload.clear();
}

/// A [`wgpu::Queue`] that counts the bytes written through it and optionally coalesces small
/// 4-byte-aligned buffer writes into a single per-frame staging upload.
#[derive(Debug)]
pub struct UploadQueue {
    queue: wgpu::Queue,
    written: AtomicU64,
    staging_enabled: bool,
    has_staged: AtomicBool,
    staging: Mutex<StagingBeltState>,
}

impl UploadQueue {
    /// Wraps a queue without write coalescing (used for standalone/custom temporary queues).
    pub fn new(queue: wgpu::Queue) -> Self {
        Self {
            queue,
            written: AtomicU64::new(0),
            staging_enabled: false,
            has_staged: AtomicBool::new(false),
            staging: Mutex::new(StagingBeltState::default()),
        }
    }

    /// Wraps a queue with `< 64 KiB` aligned buffer write coalescing enabled.
    pub fn with_staging(queue: wgpu::Queue) -> Self {
        Self {
            queue,
            written: AtomicU64::new(0),
            staging_enabled: true,
            has_staged: AtomicBool::new(false),
            staging: Mutex::new(StagingBeltState::default()),
        }
    }

    /// Returns whether small-write staging should be enabled for `(_is_wasm32, backend)`.
    ///
    /// All non-GL backends (WebGPU, Metal, Vulkan, DX12) coalesce `< 64 KiB` aligned buffer
    /// writes into a single persistent staging buffer and record `copy_buffer_to_buffer` transfers
    /// in the frame command encoder, avoiding per-write `wgpu` staging buffer / `PendingWrites`
    /// churn. WebGL2 (`Backend::Gl`) forbids `copyBufferSubData` between non-element staging
    /// buffers and `ELEMENT_ARRAY_BUFFER` destinations (WebGL2 §5.14.5), so direct `write_buffer`
    /// must be used.
    pub(crate) const fn should_stage_for_backend(_is_wasm32: bool, backend: wgpu::Backend) -> bool {
        !matches!(backend, wgpu::Backend::Gl)
    }

    /// Wraps `queue` using per-frame staging only when the target architecture and backend support
    /// copying from a `COPY_SRC` staging buffer into any buffer usage (including index buffers).
    pub fn for_backend(queue: wgpu::Queue, backend: wgpu::Backend) -> Self {
        if Self::should_stage_for_backend(cfg!(target_arch = "wasm32"), backend) {
            Self::with_staging(queue)
        } else {
            Self::new(queue)
        }
    }

    /// Writes `data` into `buffer` at `offset`, counting its bytes.
    ///
    /// When staging is enabled and `data` is `< 64 KiB` with 4-byte-aligned `offset` and length,
    /// the write is appended into CPU staging memory and flushed in one `writeBuffer` + GPU
    /// `copy_buffer_to_buffer` command buffer right before frame submission.
    pub fn write_buffer(&self, buffer: &wgpu::Buffer, offset: wgpu::BufferAddress, data: &[u8]) {
        self.written.fetch_add(data.len() as u64, Ordering::Relaxed);
        if data.is_empty() {
            return;
        }
        let size = data.len() as u64;
        let can_stage = self.staging_enabled
            && data.len() < MAX_STAGED_WRITE_BYTES
            && offset.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT)
            && size.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT);

        if can_stage {
            let mut staging = crate::platform::lock_unpoisoned(&self.staging);
            let StagingBeltState {
                payload, copies, ..
            } = &mut *staging;
            if let Some(last) = copies.last_mut() {
                if &last.dst == buffer && last.dst_offset == offset && last.size == size {
                    let start = last.src_offset as usize;
                    if let Some(dst) = payload.get_mut(start..start + data.len()) {
                        dst.copy_from_slice(data);
                    }
                    return;
                }
                if &last.dst == buffer
                    && last.dst_offset + last.size == offset
                    && (last.src_offset + last.size) as usize == payload.len()
                    && payload.len() + data.len() <= MAX_STAGING_BUFFER_CAPACITY as usize
                {
                    payload.extend_from_slice(data);
                    last.size += size;
                    return;
                }
            }
            if payload.len() + data.len() <= MAX_STAGING_BUFFER_CAPACITY as usize {
                let src_offset = payload.len() as u64;
                payload.extend_from_slice(data);
                copies.push(StagedCopy {
                    dst: buffer.clone(),
                    src_offset,
                    dst_offset: offset,
                    size,
                });
                self.has_staged.store(true, Ordering::Release);
                return;
            }
        }

        if self.staging_enabled {
            self.flush_staged_for_buffer(buffer);
        }
        #[expect(
            clippy::disallowed_methods,
            reason = "UploadQueue::write_buffer is the counted upload path"
        )]
        self.queue.write_buffer(buffer, offset, data);
    }

    /// Cancels any pending staged writes targeting `buffer` before the caller destroys it.
    pub fn cancel_staged_buffer(&self, buffer: &wgpu::Buffer) {
        if !self.staging_enabled || !self.has_staged.load(Ordering::Acquire) {
            return;
        }
        let mut staging = crate::platform::lock_unpoisoned(&self.staging);
        staging.copies.retain(|copy| &copy.dst != buffer);
        if staging.copies.is_empty() {
            staging.payload.clear();
            self.has_staged.store(false, Ordering::Release);
        }
    }

    /// Encodes all staged writes into a reusable staging chunk (`NativeStagingRing` on native, or
    /// a `write_buffer`-updated staging belt on wasm32) and records `copy_buffer_to_buffer`
    /// transfers onto `encoder`. Returns `true` when at least one staged copy was recorded.
    pub fn encode_staged_into(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
    ) -> bool {
        if !self.staging_enabled {
            return false;
        }
        let mut staging = crate::platform::lock_unpoisoned(&self.staging);
        #[cfg(not(target_arch = "wasm32"))]
        {
            if staging.copies.is_empty() {
                staging.payload.clear();
                staging.native_ring.record_quiet_frame();
                staging
                    .payload
                    .shrink_to(SHRINK_STAGING_BUFFER_CAPACITY as usize);
                staging.copies.shrink_to(256);
                self.has_staged.store(false, Ordering::Release);
                return false;
            }
            let max_buf = *staging.cached_max_buf.get_or_insert_with(|| {
                device
                    .limits()
                    .max_buffer_size
                    .min(MAX_STAGING_BUFFER_CAPACITY)
            });
            let StagingBeltState {
                payload,
                copies,
                native_ring,
                ..
            } = &mut *staging;
            let encoded = if native_ring.encode_into(device, encoder, payload, copies, max_buf) {
                payload.clear();
                if payload.capacity() > SHRINK_STAGING_BUFFER_CAPACITY as usize * 4 {
                    payload.shrink_to(SHRINK_STAGING_BUFFER_CAPACITY as usize);
                    copies.shrink_to(256);
                }
                true
            } else {
                flush_copies_direct(&self.queue, payload, copies);
                false
            };
            self.has_staged
                .store(!staging.copies.is_empty(), Ordering::Release);
            encoded
        }
        #[cfg(target_arch = "wasm32")]
        {
            let StagingBeltState {
                payload,
                copies,
                wasm_belt,
            } = &mut *staging;
            let encoded = wasm_belt.encode_into(&self.queue, device, encoder, payload, copies);
            self.has_staged.store(!copies.is_empty(), Ordering::Release);
            encoded
        }
    }

    /// Uploads all staged `< 64 KiB` writes via a single `wgpu::Queue::write_buffer` into the
    /// persistent staging buffer and returns a command buffer encoding the `copy_buffer_to_buffer`
    /// transfers to submit ahead of the frame's render passes.
    pub fn flush_staged(&self, device: &wgpu::Device) -> Option<wgpu::CommandBuffer> {
        if !self.staging_enabled
            || !self.has_staged.load(Ordering::Acquire)
            || crate::platform::lock_unpoisoned(&self.staging)
                .copies
                .is_empty()
        {
            return None;
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: cfg!(debug_assertions).then_some("upload_queue_staging_encoder"),
        });
        self.encode_staged_into(device, &mut encoder)
            .then(|| encoder.finish())
    }

    fn flush_staged_for_buffer(&self, target: &wgpu::Buffer) {
        if !self.has_staged.load(Ordering::Acquire) {
            return;
        }
        let mut staging = crate::platform::lock_unpoisoned(&self.staging);
        if !staging.copies.iter().any(|copy| &copy.dst == target) {
            return;
        }
        let StagingBeltState {
            payload, copies, ..
        } = &mut *staging;
        copies.retain(|copy| {
            if &copy.dst == target {
                let start = copy.src_offset as usize;
                let end = start + copy.size as usize;
                if let Some(slice) = payload.get(start..end) {
                    #[expect(
                        clippy::disallowed_methods,
                        reason = "UploadQueue flushes staged writes for a target buffer after counting bytes"
                    )]
                    self.queue
                        .write_buffer(&copy.dst, copy.dst_offset, slice);
                }
                false
            } else {
                true
            }
        });
        if copies.is_empty() {
            payload.clear();
            self.has_staged.store(false, Ordering::Release);
        }
    }

    /// Writes `data` into a texture, counting its bytes.
    pub fn write_texture(
        &self,
        texture: wgpu::TexelCopyTextureInfo<'_>,
        data: &[u8],
        layout: wgpu::TexelCopyBufferLayout,
        size: wgpu::Extent3d,
    ) {
        self.written.fetch_add(data.len() as u64, Ordering::Relaxed);
        #[expect(
            clippy::disallowed_methods,
            reason = "UploadQueue::write_texture is the counted upload path"
        )]
        self.queue.write_texture(texture, data, layout, size);
    }

    /// Creates a buffer holding `descriptor.contents`, counting its bytes.
    pub fn create_buffer_init(
        &self,
        device: &wgpu::Device,
        descriptor: &wgpu::util::BufferInitDescriptor<'_>,
    ) -> wgpu::Buffer {
        self.written
            .fetch_add(descriptor.contents.len() as u64, Ordering::Relaxed);
        #[expect(
            clippy::disallowed_methods,
            reason = "UploadQueue::create_buffer_init is the counted upload path"
        )]
        device.create_buffer_init(descriptor)
    }

    /// Creates a texture holding `data`, counting its bytes.
    pub fn create_texture_with_data(
        &self,
        device: &wgpu::Device,
        descriptor: &wgpu::TextureDescriptor<'_>,
        order: wgpu::util::TextureDataOrder,
        data: &[u8],
    ) -> wgpu::Texture {
        self.written.fetch_add(data.len() as u64, Ordering::Relaxed);
        #[expect(
            clippy::disallowed_methods,
            reason = "UploadQueue::create_texture_with_data is the counted upload path"
        )]
        device.create_texture_with_data(&self.queue, descriptor, order, data)
    }

    /// Submits command buffers, flushing any remaining direct-submission staged buffer writes first
    /// and re-mapping native staging chunks after submission.
    pub fn submit<I: IntoIterator<Item = wgpu::CommandBuffer>>(
        &self,
        command_buffers: I,
    ) -> wgpu::SubmissionIndex {
        if self.staging_enabled && self.has_staged.load(Ordering::Acquire) {
            let mut staging = crate::platform::lock_unpoisoned(&self.staging);
            if !staging.copies.is_empty() {
                let StagingBeltState {
                    payload, copies, ..
                } = &mut *staging;
                flush_copies_direct(&self.queue, payload, copies);
            }
            self.has_staged.store(false, Ordering::Release);
        }
        let submission = self.queue.submit(command_buffers);
        #[cfg(not(target_arch = "wasm32"))]
        if self.staging_enabled {
            crate::platform::lock_unpoisoned(&self.staging)
                .native_ring
                .recall_after_submit();
        }
        submission
    }

    /// Presents a surface texture.
    pub fn present(&self, surface_texture: wgpu::SurfaceTexture) {
        self.queue.present(surface_texture);
    }

    /// Nanoseconds per timestamp query tick.
    pub fn get_timestamp_period(&self) -> f32 {
        self.queue.get_timestamp_period()
    }

    /// The bytes written since the last call.
    pub fn take_written_bytes(&self) -> u64 {
        self.written.swap(0, Ordering::Relaxed)
    }

    /// The wrapped queue, for the wgpu calls that take one without uploading through it.
    pub fn inner(&self) -> &wgpu::Queue {
        &self.queue
    }
}

#[cfg(test)]
mod tests;
