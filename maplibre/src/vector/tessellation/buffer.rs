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
    uploaded_vertices: usize,
    icon_bits: Vec<u64>,
    shared_geometry_key: Option<u64>,
}

impl<V, I> OverAlignedVertexBuffer<V, I> {
    /// Creates empty geometry with no drawable indices.
    pub fn empty() -> Self {
        Self {
            buffer: VertexBuffers::with_capacity(0, 0),
            usable_indices: 0,
            uploaded_vertices: 0,
            icon_bits: Vec::new(),
            shared_geometry_key: None,
        }
    }

    /// Wraps pre-allocated, copy-aligned vertex and index vectors without copying.
    pub fn from_vecs(vertices: Vec<V>, indices: Vec<I>, usable_indices: u32) -> Self {
        Self {
            buffer: VertexBuffers { vertices, indices },
            usable_indices,
            uploaded_vertices: 0,
            icon_bits: Vec::new(),
            shared_geometry_key: None,
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
            uploaded_vertices: 0,
            icon_bits: Vec::new(),
            shared_geometry_key: None,
        }
    }

    /// Attaches a batch-scoped shared vertex-allocation key so sibling style layers on the same
    /// source layer reuse a single uploaded GPU vertex buffer.
    #[inline]
    pub fn with_shared_geometry_key(mut self, key: Option<u64>) -> Self {
        self.shared_geometry_key = key;
        self
    }

    /// Shared vertex-allocation key when multiple style layers reference one source-layer vertex slab.
    #[inline]
    pub fn shared_geometry_key(&self) -> Option<u64> {
        self.shared_geometry_key
    }

    /// Total GPU vertex count, preserved even after CPU vertex storage is released post-upload.
    pub fn vertex_count(&self) -> usize {
        if self.buffer.vertices.is_empty() {
            self.uploaded_vertices
        } else {
            self.buffer.vertices.len()
        }
    }

    pub(crate) fn icon_bits_bytes(&self) -> usize {
        self.icon_bits.capacity() * std::mem::size_of::<u64>()
    }
}

impl<I> OverAlignedVertexBuffer<crate::render::shaders::ShaderSymbolVertex, I> {
    pub(crate) fn is_icon_vertex(&self, index: usize) -> bool {
        if let Some(vertex) = self.buffer.vertices.get(index) {
            matches!(vertex.kind(), 1 | 2)
        } else {
            self.icon_bits
                .get(index / 64)
                .is_some_and(|word| ((word >> (index % 64)) & 1) != 0)
        }
    }

    /// Compacts per-vertex icon/text kind flags into 1 bit per vertex and frees CPU vertex storage
    /// once `SymbolBufferPool` holds the GPU copy.
    pub(crate) fn release_uploaded_symbol_vertices(&mut self) {
        if self.buffer.vertices.is_empty() {
            return;
        }
        let count = self.buffer.vertices.len();
        let mut bits = vec![0_u64; count.div_ceil(64)];
        for (i, vertex) in self.buffer.vertices.iter().enumerate() {
            if matches!(vertex.kind(), 1 | 2) {
                bits[i / 64] |= 1_u64 << (i % 64);
            }
        }
        self.uploaded_vertices = count;
        self.icon_bits = bits;
        self.buffer.vertices = Vec::new();
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
            uploaded_vertices: 0,
            icon_bits: Vec::new(),
            shared_geometry_key: None,
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
