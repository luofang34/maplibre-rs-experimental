//! GPU copy-aligned vertex and index buffers produced by tessellation.

use std::sync::Arc;

use bytemuck::Pod;
use lyon::tessellation::VertexBuffers;

/// Geometry with a draw-index count separate from any GPU copy-alignment padding.
/// Worker pre-packed GPU geometry ready for zero-conversion buffer upload.
#[derive(Clone, Debug)]
pub struct PrepackedGpuGeometry {
    /// Pre-packed 16-byte GPU vertices (or packed 8-byte compact extrusion pairs when
    /// [`Self::compact_extrusion`] is set) matching [`crate::render::shaders::GpuShaderVertex`].
    pub vertices: Vec<crate::render::shaders::GpuShaderVertex>,
    /// Optional pre-narrowed `u16` indices when all vertices fit in 16 bits.
    pub indices_u16: Option<Vec<u16>>,
    /// Optional pre-packed `u32` indices when vertices exceed 16-bit addressing.
    pub indices_u32: Option<Vec<u32>>,
    /// True when [`Self::vertices`] stores two 8-byte compact extrusion vertices per word.
    pub compact_extrusion: bool,
    /// Logical vertex count referenced by indices (equals `vertices.len()` unless compact).
    pub logical_vertex_count: u32,
}

impl PrepackedGpuGeometry {
    /// Returns the logical vertex count referenced by indices.
    #[inline]
    pub fn logical_len(&self) -> usize {
        if self.logical_vertex_count > 0 {
            self.logical_vertex_count as usize
        } else {
            self.vertices.len()
        }
    }
}

/// Conversion from `VertexBuffers` pads indices and requires copy-aligned vertex bytes.
#[derive(Clone)]
pub struct OverAlignedVertexBuffer<V, I> {
    /// Vertex and index storage, including any upload padding.
    pub buffer: VertexBuffers<V, I>,
    /// Number of leading indices to draw, excluding padding.
    pub usable_indices: u32,
    uploaded_vertices: u32,
    icon_bits: Vec<u64>,
    shared_geometry_key: Option<u64>,
    prepacked_gpu: Option<Arc<PrepackedGpuGeometry>>,
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
            prepacked_gpu: None,
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
            prepacked_gpu: None,
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
            prepacked_gpu: None,
        }
    }

    /// Attaches a batch-scoped shared vertex-allocation key so sibling style layers on the same
    /// source layer reuse a single uploaded GPU vertex buffer.
    #[inline]
    pub fn with_shared_geometry_key(mut self, key: Option<u64>) -> Self {
        self.shared_geometry_key = key;
        self
    }

    /// Overwrites the shared vertex-allocation key attached to this buffer.
    #[inline]
    pub fn set_shared_geometry_key(&mut self, key: Option<u64>) {
        self.shared_geometry_key = key;
    }

    /// Shared vertex-allocation key when multiple style layers reference one source-layer vertex slab.
    #[inline]
    pub fn shared_geometry_key(&self) -> Option<u64> {
        self.shared_geometry_key
    }

    /// Attaches worker pre-packed GPU vertices and optional `u16` indices.
    pub fn with_prepacked_gpu(mut self, prepacked: PrepackedGpuGeometry) -> Self {
        let len = u32::try_from(prepacked.logical_len()).unwrap_or(u32::MAX);
        self.uploaded_vertices = self.uploaded_vertices.max(len);
        self.prepacked_gpu = Some(Arc::new(prepacked));
        self
    }

    /// Returns worker pre-packed GPU geometry when present.
    #[inline]
    pub fn prepacked_gpu(&self) -> Option<&PrepackedGpuGeometry> {
        self.prepacked_gpu.as_deref()
    }

    /// Releases worker pre-packed GPU geometry after upload.
    pub fn clear_prepacked_gpu(&mut self) {
        if let Some(prepacked) = self.prepacked_gpu.take() {
            let len = u32::try_from(prepacked.logical_len()).unwrap_or(u32::MAX);
            self.uploaded_vertices = self.uploaded_vertices.max(len);
        }
    }

    /// Takes shared ownership of any attached pre-packed GPU geometry while preserving `vertex_count()`.
    pub fn take_prepacked_gpu_arc(&mut self) -> Option<Arc<PrepackedGpuGeometry>> {
        let prepacked = self.prepacked_gpu.take()?;
        let len = u32::try_from(prepacked.logical_len()).unwrap_or(u32::MAX);
        self.uploaded_vertices = self.uploaded_vertices.max(len);
        Some(prepacked)
    }

    /// Takes ownership of any attached pre-packed GPU geometry while preserving `vertex_count()`.
    pub fn take_prepacked_gpu(&mut self) -> Option<PrepackedGpuGeometry> {
        let prepacked = self.take_prepacked_gpu_arc()?;
        Some(Arc::try_unwrap(prepacked).unwrap_or_else(|shared| (*shared).clone()))
    }

    /// Number of index elements currently stored in CPU memory (`u16` prepacked, `u32` prepacked, or raw).
    #[inline]
    pub fn cpu_index_len(&self) -> usize {
        self.prepacked_gpu
            .as_ref()
            .map_or(self.buffer.indices.len(), |p| {
                p.indices_u16.as_ref().map_or_else(
                    || {
                        p.indices_u32
                            .as_ref()
                            .map_or(self.buffer.indices.len(), Vec::len)
                    },
                    Vec::len,
                )
            })
    }

    /// Byte capacity of pre-packed GPU buffers currently held in CPU memory.
    #[inline]
    pub fn prepacked_bytes(&self) -> usize {
        self.prepacked_gpu.as_ref().map_or(0, |p| {
            p.vertices.capacity() * std::mem::size_of::<crate::render::shaders::GpuShaderVertex>()
                + p.indices_u16
                    .as_ref()
                    .map_or(0, |idx| idx.capacity() * std::mem::size_of::<u16>())
                + p.indices_u32
                    .as_ref()
                    .map_or(0, |idx| idx.capacity() * std::mem::size_of::<u32>())
        })
    }

    /// Returns `true` while raw or pre-packed vertices remain in CPU memory.
    #[inline]
    pub fn has_cpu_vertices(&self) -> bool {
        !self.buffer.vertices.is_empty()
            || self
                .prepacked_gpu
                .as_ref()
                .is_some_and(|p| !p.vertices.is_empty())
    }

    /// Returns `true` while raw or pre-packed indices remain in CPU memory.
    #[inline]
    pub fn has_cpu_indices(&self) -> bool {
        !self.buffer.indices.is_empty()
            || self.prepacked_gpu.as_ref().is_some_and(|p| {
                p.indices_u16.as_ref().is_some_and(|idx| !idx.is_empty())
                    || p.indices_u32.as_ref().is_some_and(|idx| !idx.is_empty())
            })
    }

    /// Returns `true` when this buffer carries non-empty drawable geometry.
    #[inline]
    pub fn has_drawable_geometry(&self) -> bool {
        self.usable_indices > 0 || self.has_cpu_indices()
    }

    /// Releases CPU-side geometry after GPU upload while preserving `vertex_count()`.
    pub fn release_cpu_geometry(&mut self) {
        let count = u32::try_from(self.vertex_count()).unwrap_or(u32::MAX);
        self.uploaded_vertices = self.uploaded_vertices.max(count);
        self.buffer.vertices = Vec::new();
        self.buffer.indices = Vec::new();
        self.prepacked_gpu = None;
    }

    /// Total GPU vertex count, preserved even after CPU vertex storage is released post-upload.
    pub fn vertex_count(&self) -> usize {
        if let Some(prepacked) = &self.prepacked_gpu {
            if !prepacked.vertices.is_empty() {
                return prepacked.logical_len();
            }
        }
        if self.buffer.vertices.is_empty() {
            self.uploaded_vertices as usize
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
            if matches!(vertex.kind(), 1 | 2)
                && let Some(word) = bits.get_mut(i / 64)
            {
                *word |= 1_u64 << (i % 64);
            }
        }
        self.uploaded_vertices = u32::try_from(count).unwrap_or(u32::MAX);
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
            prepacked_gpu: None,
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
        if stride == 0 {
            return;
        }
        while !(self.vertices.len() as wgpu::BufferAddress * stride).is_multiple_of(align) {
            self.vertices.push(V::zeroed());
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

impl super::ZeroTessellator<crate::vector::IndexDataType> {
    /// Finalizes tessellated geometry into an upload-ready [`OverAlignedVertexBuffer`].
    pub(crate) fn into_over_aligned_buffer(
        mut self,
    ) -> OverAlignedVertexBuffer<crate::render::ShaderVertex, crate::vector::IndexDataType> {
        if let Some(mut prepack) = self.streaming_prepack.take() {
            if !self.buffer.vertices.is_empty() {
                prepack.push_vertices(&self.buffer.vertices);
                self.buffer.vertices = Vec::new();
            }
            return prepack.finish(self.buffer.indices);
        }
        self.buffer.vertices.shrink_to_fit();
        self.buffer.indices.shrink_to_fit();
        self.buffer.into()
    }
}
