//! GPU copy-aligned vertex and index buffers produced by tessellation.

use bytemuck::Pod;
use lyon::tessellation::VertexBuffers;

/// Geometry with a draw-index count separate from any GPU copy-alignment padding.
/// Conversion from `VertexBuffers` pads indices and requires copy-aligned vertex bytes.
#[derive(Clone)]
pub struct OverAlignedVertexBuffer<V, I> {
    /// Vertex and index storage, including any upload padding.
    pub buffer: VertexBuffers<V, I>,
    /// Number of leading indices to draw, excluding padding.
    pub usable_indices: u32,
}

impl<V, I> OverAlignedVertexBuffer<V, I> {
    /// Creates empty geometry with no drawable indices.
    pub fn empty() -> Self {
        Self {
            buffer: VertexBuffers::with_capacity(0, 0),
            usable_indices: 0,
        }
    }

    /// Reconstructs buffers without adding padding or validating `usable_indices`.
    /// Callers must supply copy-aligned storage and a draw count within the index buffer.
    pub fn from_iters<IV, II>(vertices: IV, indices: II, usable_indices: u32) -> Self
    where
        IV: IntoIterator<Item = V>,
        II: IntoIterator<Item = I>,
        IV::IntoIter: ExactSizeIterator,
        II::IntoIter: ExactSizeIterator,
    {
        let vertices = vertices.into_iter();
        let indices = indices.into_iter();
        let mut buffers = VertexBuffers::with_capacity(vertices.len(), indices.len());
        buffers.vertices.extend(vertices);
        buffers.indices.extend(indices);
        Self {
            buffer: buffers,
            usable_indices,
        }
    }
}

impl<V: Pod, I: Pod> From<VertexBuffers<V, I>> for OverAlignedVertexBuffer<V, I> {
    fn from(mut buffer: VertexBuffers<V, I>) -> Self {
        let usable_indices = buffer.indices.len() as u32;
        buffer.align_vertices();
        buffer.align_indices();
        Self {
            buffer,
            usable_indices,
        }
    }
}

trait Align<V: Pod, I: Pod> {
    fn align_vertices(&mut self);
    fn align_indices(&mut self);
}

impl<V: Pod, I: Pod> Align<V, I> for VertexBuffers<V, I> {
    fn align_vertices(&mut self) {
        let align = wgpu::COPY_BUFFER_ALIGNMENT;
        let stride = std::mem::size_of::<V>() as wgpu::BufferAddress;
        let unpadded_bytes = self.vertices.len() as wgpu::BufferAddress * stride;
        let padding_bytes = (align - unpadded_bytes % align) % align;

        if padding_bytes != 0 {
            panic!(
                "vertices are always aligned to wgpu::COPY_BUFFER_ALIGNMENT \
                    because GpuVertexUniform is aligned"
            )
        }
    }

    fn align_indices(&mut self) {
        let align = wgpu::COPY_BUFFER_ALIGNMENT;
        let stride = std::mem::size_of::<I>() as wgpu::BufferAddress;
        let unpadded_bytes = self.indices.len() as wgpu::BufferAddress * stride;
        let padding_bytes = (align - unpadded_bytes % align) % align;
        let overpad = padding_bytes.div_ceil(stride);

        for _ in 0..overpad {
            self.indices.push(I::zeroed());
        }
    }
}
