//! The renderer's queue, counting the bytes each frame writes to buffers and textures.
//!
//! Every write goes through [`UploadQueue::write_buffer`] or [`UploadQueue::write_texture`],
//! which shadow the [`wgpu::Queue`] methods of the same name; everything else reaches the inner
//! queue through `Deref`.

use std::sync::atomic::{AtomicU64, Ordering};

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
        self.queue.write_texture(texture, data, layout, size);
    }

    /// The bytes written since the last call.
    pub fn take_written_bytes(&self) -> u64 {
        self.written.swap(0, Ordering::Relaxed)
    }

    /// The wrapped queue.
    pub fn inner(&self) -> &wgpu::Queue {
        &self.queue
    }
}

impl std::ops::Deref for UploadQueue {
    type Target = wgpu::Queue;

    fn deref(&self) -> &wgpu::Queue {
        &self.queue
    }
}

#[cfg(test)]
mod tests;
