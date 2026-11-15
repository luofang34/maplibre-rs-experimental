//! The renderer's queue, counting the bytes each frame writes to buffers and textures, and
//! coalescing small aligned buffer writes into one per-frame staging upload.
//!
//! Bytes reach the GPU through [`UploadQueue::write_buffer`], [`UploadQueue::write_texture`],
//! [`UploadQueue::create_buffer_init`] and [`UploadQueue::create_texture_with_data`]. The queue
//! is not reachable as a [`wgpu::Queue`] except through [`UploadQueue::inner`], and the
//! crate denies the uncounted wgpu methods that its clippy.toml lists, so clippy rejects an
//! upload that skips the count.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Mutex,
};

use wgpu::util::DeviceExt;

/// Maximum single `write_buffer` payload (exclusive) staged into the per-frame coalescing belt.
const MAX_STAGED_WRITE_BYTES: usize = 65_536;

/// Minimum persistent GPU staging buffer capacity (`64 KiB`).
const MIN_STAGING_BUFFER_CAPACITY: u64 = 64 * 1024;

/// Steady-state post-burst staging buffer ceiling (`256 KiB`).
const SHRINK_STAGING_BUFFER_CAPACITY: u64 = 256 * 1024;

/// Hard upper bound on the persistent coalescing staging buffer (`2 MiB`) so initial tile upload
/// bursts never pin tens of megabytes of transient staging memory.
const MAX_STAGING_BUFFER_CAPACITY: u64 = 2 * 1024 * 1024;

/// Consecutive low-volume frames before a burst-sized staging buffer shrinks back to `256 KiB`.
const QUIET_FRAMES_BEFORE_SHRINK: u8 = 8;

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
    buffer: Option<(wgpu::Buffer, u64)>,
    quiet_frames: u8,
}

impl Drop for StagingBeltState {
    fn drop(&mut self) {
        if let Some((buf, _)) = self.buffer.take() {
            buf.destroy();
        }
    }
}

/// A [`wgpu::Queue`] that counts the bytes written through it and optionally coalesces small
/// 4-byte-aligned buffer writes into a single per-frame staging upload.
#[derive(Debug)]
pub struct UploadQueue {
    queue: wgpu::Queue,
    written: AtomicU64,
    staging_enabled: bool,
    staging: Mutex<StagingBeltState>,
}

impl UploadQueue {
    /// Wraps a queue without write coalescing (used for standalone/custom temporary queues).
    pub fn new(queue: wgpu::Queue) -> Self {
        Self {
            queue,
            written: AtomicU64::new(0),
            staging_enabled: false,
            staging: Mutex::new(StagingBeltState::default()),
        }
    }

    /// Wraps a queue with `< 64 KiB` aligned buffer write coalescing enabled.
    pub fn with_staging(queue: wgpu::Queue) -> Self {
        Self {
            queue,
            written: AtomicU64::new(0),
            staging_enabled: true,
            staging: Mutex::new(StagingBeltState::default()),
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
            if let Some(last) = staging.copies.last() {
                if &last.dst == buffer && last.dst_offset == offset && last.size == size {
                    let start = last.src_offset as usize;
                    staging.payload[start..start + data.len()].copy_from_slice(data);
                    return;
                }
            }
            if staging.payload.len() + data.len() <= MAX_STAGING_BUFFER_CAPACITY as usize {
                let src_offset = staging.payload.len() as u64;
                staging.payload.extend_from_slice(data);
                staging.copies.push(StagedCopy {
                    dst: buffer.clone(),
                    src_offset,
                    dst_offset: offset,
                    size,
                });
                return;
            }
        }

        if self.staging_enabled {
            self.flush_staged_for_buffer(buffer);
        }
        #[allow(clippy::disallowed_methods)] // The counted path itself.
        self.queue.write_buffer(buffer, offset, data);
    }

    /// Cancels any pending staged writes targeting `buffer` before the caller destroys it.
    pub fn cancel_staged_buffer(&self, buffer: &wgpu::Buffer) {
        if !self.staging_enabled {
            return;
        }
        let mut staging = crate::platform::lock_unpoisoned(&self.staging);
        staging.copies.retain(|copy| &copy.dst != buffer);
        if staging.copies.is_empty() {
            staging.payload.clear();
        }
    }

    /// Uploads all staged `< 64 KiB` writes via a single `wgpu::Queue::write_buffer` into the
    /// persistent staging buffer and returns a command buffer encoding the `copy_buffer_to_buffer`
    /// transfers to submit ahead of the frame's render passes.
    pub fn flush_staged(&self, device: &wgpu::Device) -> Option<wgpu::CommandBuffer> {
        if !self.staging_enabled {
            return None;
        }
        let mut staging = crate::platform::lock_unpoisoned(&self.staging);
        if staging.copies.is_empty() {
            staging.payload.clear();
            if staging
                .buffer
                .as_ref()
                .is_some_and(|(_, cap)| *cap > SHRINK_STAGING_BUFFER_CAPACITY)
            {
                staging.quiet_frames = staging.quiet_frames.saturating_add(1);
                if staging.quiet_frames >= QUIET_FRAMES_BEFORE_SHRINK {
                    if let Some((old_buf, _)) = staging.buffer.take() {
                        old_buf.destroy();
                    }
                    staging
                        .payload
                        .shrink_to(SHRINK_STAGING_BUFFER_CAPACITY as usize);
                    staging.copies.shrink_to(256);
                    staging.quiet_frames = 0;
                }
            }
            return None;
        }

        let payload_bytes = staging.payload.len() as u64;
        if payload_bytes <= SHRINK_STAGING_BUFFER_CAPACITY {
            staging.quiet_frames = staging.quiet_frames.saturating_add(1);
        } else {
            staging.quiet_frames = 0;
        }
        let max_buf = device
            .limits()
            .max_buffer_size
            .min(MAX_STAGING_BUFFER_CAPACITY);
        if payload_bytes > max_buf {
            let StagingBeltState {
                payload, copies, ..
            } = &mut *staging;
            for copy in copies.drain(..) {
                let start = copy.src_offset as usize;
                let end = start + copy.size as usize;
                #[allow(clippy::disallowed_methods)]
                self.queue
                    .write_buffer(&copy.dst, copy.dst_offset, &payload[start..end]);
            }
            payload.clear();
            return None;
        }

        let should_shrink = staging.quiet_frames >= QUIET_FRAMES_BEFORE_SHRINK
            && staging
                .buffer
                .as_ref()
                .is_some_and(|(_, cap)| *cap > SHRINK_STAGING_BUFFER_CAPACITY);
        let need_alloc = should_shrink
            || staging
                .buffer
                .as_ref()
                .is_none_or(|(_, cap)| *cap < payload_bytes);
        if need_alloc {
            let next_cap = if should_shrink {
                staging.quiet_frames = 0;
                SHRINK_STAGING_BUFFER_CAPACITY.min(max_buf)
            } else {
                payload_bytes
                    .max(MIN_STAGING_BUFFER_CAPACITY)
                    .checked_next_power_of_two()
                    .unwrap_or(max_buf)
                    .min(max_buf)
            };
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("upload_queue_staging_buffer"),
                size: next_cap,
                usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            if let Some((old_buf, _)) = staging.buffer.replace((buf, next_cap)) {
                old_buf.destroy();
            }
        }

        let (staging_buf, _) = staging.buffer.as_ref()?;
        #[allow(clippy::disallowed_methods)]
        self.queue.write_buffer(staging_buf, 0, &staging.payload);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("upload_queue_staging_encoder"),
        });
        let StagingBeltState {
            payload,
            copies,
            buffer,
            ..
        } = &mut *staging;
        if let Some((staging_buf, _)) = buffer.as_ref() {
            for copy in copies.drain(..) {
                encoder.copy_buffer_to_buffer(
                    staging_buf,
                    copy.src_offset,
                    &copy.dst,
                    copy.dst_offset,
                    copy.size,
                );
            }
        }
        payload.clear();
        if should_shrink {
            payload.shrink_to(SHRINK_STAGING_BUFFER_CAPACITY as usize);
            copies.shrink_to(256);
        }
        Some(encoder.finish())
    }

    fn flush_staged_for_buffer(&self, target: &wgpu::Buffer) {
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
                #[allow(clippy::disallowed_methods)]
                self.queue
                    .write_buffer(&copy.dst, copy.dst_offset, &payload[start..end]);
                false
            } else {
                true
            }
        });
        if copies.is_empty() {
            payload.clear();
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
        #[allow(clippy::disallowed_methods)] // The counted path itself.
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
        #[allow(clippy::disallowed_methods)] // The counted path itself.
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
        #[allow(clippy::disallowed_methods)] // The counted path itself.
        device.create_texture_with_data(&self.queue, descriptor, order, data)
    }

    /// Submits command buffers, flushing any remaining direct-submission staged buffer writes first.
    pub fn submit<I: IntoIterator<Item = wgpu::CommandBuffer>>(
        &self,
        command_buffers: I,
    ) -> wgpu::SubmissionIndex {
        if self.staging_enabled {
            let mut staging = crate::platform::lock_unpoisoned(&self.staging);
            if !staging.copies.is_empty() {
                let StagingBeltState {
                    payload, copies, ..
                } = &mut *staging;
                for copy in copies.drain(..) {
                    let start = copy.src_offset as usize;
                    let end = start + copy.size as usize;
                    #[allow(clippy::disallowed_methods)]
                    self.queue
                        .write_buffer(&copy.dst, copy.dst_offset, &payload[start..end]);
                }
                payload.clear();
            }
        }
        self.queue.submit(command_buffers)
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
