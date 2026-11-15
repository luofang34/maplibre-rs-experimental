//! GPU allocations, render targets, shader interfaces and pipeline construction.

#![deny(missing_docs)]

pub use buffer::*;
pub use mipmap::*;
pub use pipeline::*;
pub(crate) use pipeline_cache::create_bind_group_layout_cached;
pub use pipeline_cache::SharedPipelines;
pub use shader::*;
pub(crate) use shared::share_gpu;
pub use surface::*;
pub use texture::*;
pub use tile_pipeline::*;

mod buffer;
mod mipmap;
mod pipeline;
mod pipeline_cache;
mod shader;
mod shared;
mod surface;
mod texture;
mod tile_pipeline;

/// Upload interface shared by GPU queues and test backends that own a different buffer type.
pub trait Queue<B> {
    /// Copies bytes into `buffer` at a byte offset, subject to the backend's bounds and alignment.
    /// Implementations must consume or copy `data` before returning; GPU execution may be deferred.
    fn write_buffer(&self, buffer: &B, offset: wgpu::BufferAddress, data: &[u8]);

    /// Cancels any deferred staged writes targeting `buffer` before `buffer` is destroyed.
    fn cancel_pending_writes(&self, _buffer: &B) {}
}

impl Queue<wgpu::Buffer> for super::upload_queue::UploadQueue {
    fn write_buffer(&self, buffer: &wgpu::Buffer, offset: wgpu::BufferAddress, data: &[u8]) {
        super::upload_queue::UploadQueue::write_buffer(self, buffer, offset, data)
    }

    fn cancel_pending_writes(&self, buffer: &wgpu::Buffer) {
        super::upload_queue::UploadQueue::cancel_staged_buffer(self, buffer);
    }
}
