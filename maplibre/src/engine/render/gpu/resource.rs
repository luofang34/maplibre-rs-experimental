//! GPU allocations, render targets, shader interfaces and pipeline construction.

#![deny(missing_docs)]

use std::sync::Arc;

pub use buffer_pool::{BufferBindPass, LayerGeometryUpload, UniformLayerUpload, *};
pub use mipmap::*;
pub use pipeline::*;
pub use pipeline_cache::SharedPipelines;
pub(crate) use pipeline_cache::{
    create_bind_group_layout_cached, create_shader_module_from_wgsl, PipelineCacheScope,
};
pub use shader::*;
pub use surface::*;
pub use texture::*;
pub use tile_pipeline::*;
pub use vertex_buffer::*;

pub(crate) mod buffer_pool;
mod mipmap;
mod pipeline;
mod pipeline_cache;
mod shader;
mod surface;
mod texture;
mod tile_pipeline;
pub mod vertex_buffer;

/// Backing allocation and the byte capacity available to a buffer pool.
pub struct BackingBufferDescriptor<B> {
    pub(crate) buffer: B,
    pub(crate) inner_size: wgpu::BufferAddress,
}

impl<B> BackingBufferDescriptor<B> {
    /// Transfers `buffer` to the descriptor without allocating or inspecting GPU memory.
    /// `inner_size` is the usable capacity in bytes and must not exceed the allocation's size.
    pub fn new(buffer: B, inner_size: wgpu::BufferAddress) -> Self {
        Self { buffer, inner_size }
    }
}

/// The single construction point for shared GPU objects, so the public `Arc` API is identical
/// on every target.
///
/// On wasm32 wgpu handles wrap JavaScript objects and are neither `Send` nor `Sync`; the `Arc`
/// only counts references there, and the compiler rejects any `Send` bound on such a handle.
/// Because the wrapper is generic, `clippy::arc_with_non_send_sync` does not fire at these call
/// sites on any target; the native test below pins the property the lint would otherwise check.
pub(crate) fn share_gpu<T>(value: T) -> Arc<T> {
    Arc::new(value)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::sync::Arc;

    use crate::render::resource::BufferedTextureHead;

    #[test]
    fn native_gpu_handles_cross_threads() {
        fn assert_thread_safe<T: Send + Sync>() {}
        assert_thread_safe::<Arc<wgpu::Device>>();
        assert_thread_safe::<Arc<wgpu::BindGroup>>();
        assert_thread_safe::<Arc<BufferedTextureHead>>();
    }
}

/// Upload interface shared by GPU queues and test backends that own a different buffer type.
pub trait Queue<B> {
    /// Copies bytes into `buffer` at a byte offset, subject to the backend's bounds and alignment.
    /// Implementations must consume or copy `data` before returning; GPU execution may be deferred.
    fn write_buffer(&self, buffer: &B, offset: wgpu::BufferAddress, data: &[u8]);

    /// Cancels any deferred staged writes targeting `buffer` before `buffer` is destroyed.
    fn cancel_pending_writes(&self, _buffer: &B) {}
}

impl Queue<wgpu::Buffer> for crate::render::upload_queue::UploadQueue {
    fn write_buffer(&self, buffer: &wgpu::Buffer, offset: wgpu::BufferAddress, data: &[u8]) {
        crate::render::upload_queue::UploadQueue::write_buffer(self, buffer, offset, data)
    }

    fn cancel_pending_writes(&self, buffer: &wgpu::Buffer) {
        crate::render::upload_queue::UploadQueue::cancel_staged_buffer(self, buffer);
    }
}
