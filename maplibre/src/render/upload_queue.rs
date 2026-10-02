//! The renderer's queue, counting the bytes each frame writes to buffers and textures.
//!
//! Bytes reach the GPU through [`UploadQueue::write_buffer`], [`UploadQueue::write_texture`],
//! [`UploadQueue::create_buffer_init`] and [`UploadQueue::create_texture_with_data`]. The queue
//! is not reachable as a [`wgpu::Queue`] except through [`UploadQueue::inner`], and the
//! crate denies the uncounted wgpu methods that its clippy.toml lists, so clippy rejects an
//! upload that skips the count.

use std::sync::atomic::{AtomicU64, Ordering};

use wgpu::util::DeviceExt;

/// A [`wgpu::Queue`] that counts the bytes written through it.
#[derive(Debug)]
pub struct UploadQueue {
    queue: wgpu::Queue,
    written: AtomicU64,
}

impl UploadQueue {
    /// Wraps a queue, with nothing counted yet.
    pub fn new(queue: wgpu::Queue) -> Self {
        Self {
            queue,
            written: AtomicU64::new(0),
        }
    }

    /// Writes `data` into `buffer` at `offset`, counting its bytes.
    pub fn write_buffer(&self, buffer: &wgpu::Buffer, offset: wgpu::BufferAddress, data: &[u8]) {
        self.written.fetch_add(data.len() as u64, Ordering::Relaxed);
        #[allow(clippy::disallowed_methods)] // The counted path itself.
        self.queue.write_buffer(buffer, offset, data);
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

    /// Submits command buffers.
    pub fn submit<I: IntoIterator<Item = wgpu::CommandBuffer>>(
        &self,
        command_buffers: I,
    ) -> wgpu::SubmissionIndex {
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
