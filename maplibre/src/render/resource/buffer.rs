//! Buffer ownership and byte capacities supplied to suballocation pools.

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
