//! GPU copy-aligned vertex and index buffers produced by tessellation.

use std::sync::Arc;

use bytemuck::Pod;
use lyon::tessellation::VertexBuffers;

pub mod formats;
pub mod gpu_vertex;
pub mod symbol_vertex;

/// Vertex buffers index data type (`u32`, matching `INDEX_FORMAT`).
pub type IndexDataType = u32;

/// Geometry with a draw-index count separate from any GPU copy-alignment padding.
/// Worker pre-packed GPU geometry ready for zero-conversion buffer upload.
#[derive(Clone, Debug)]
pub struct PrepackedGpuGeometry {
    /// Pre-packed 16-byte GPU vertices (or packed 8-byte compact extrusion pairs when
    /// [`Self::compact_extrusion`] is set) matching [`gpu_vertex::GpuShaderVertex`].
    pub vertices: Vec<gpu_vertex::GpuShaderVertex>,
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
        resolve_logical_vertex_count(self.logical_vertex_count, self.vertices.len())
    }

    #[inline]
    fn index_len(&self, fallback: usize) -> usize {
        self.indices_u16.as_ref().map_or_else(
            || self.indices_u32.as_ref().map_or(fallback, Vec::len),
            Vec::len,
        )
    }

    #[inline]
    fn has_indices(&self) -> bool {
        self.indices_u16.as_ref().is_some_and(|idx| !idx.is_empty())
            || self.indices_u32.as_ref().is_some_and(|idx| !idx.is_empty())
    }

    #[inline]
    fn byte_capacity(&self) -> usize {
        calc_prepacked_storage_bytes(
            self.vertices.capacity(),
            self.indices_u16.as_ref().map_or(0, Vec::capacity),
            self.indices_u32.as_ref().map_or(0, Vec::capacity),
        )
    }
}

/// Read-only slices into pre-packed GPU geometry attached to an [`OverAlignedVertexBuffer`].
#[derive(Clone, Copy)]
pub struct PrepackedSlices<'a> {
    /// Pre-packed 16-byte GPU vertices (or compact extrusion pairs).
    pub vertices: &'a [gpu_vertex::GpuShaderVertex],
    /// Optional pre-narrowed `u16` indices.
    pub indices_u16: Option<&'a [u16]>,
    /// Optional pre-packed `u32` indices.
    pub indices_u32: Option<&'a [u32]>,
    /// Whether [`Self::vertices`] stores two compact extrusion vertices per word.
    pub compact_extrusion: bool,
    /// Logical vertex count referenced by indices.
    pub logical_len: usize,
}

/// Owned CPU geometry and metadata extracted from an [`OverAlignedVertexBuffer`].
pub struct RawVertexBufferParts<V, I> {
    /// Raw CPU vertices.
    pub vertices: Vec<V>,
    /// Raw CPU indices.
    pub indices: Vec<I>,
    /// Number of leading indices to draw.
    pub usable_indices: u32,
    /// Optional shared vertex-allocation key.
    pub shared_geometry_key: Option<u64>,
    /// Optional worker pre-packed GPU geometry.
    pub prepacked_gpu: Option<Arc<PrepackedGpuGeometry>>,
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
        if self.shared_geometry_key != key {
            self.shared_geometry_key = key;
        }
    }

    /// Shared vertex-allocation key when multiple style layers reference one source-layer vertex slab.
    #[inline]
    pub fn shared_geometry_key(&self) -> Option<u64> {
        self.shared_geometry_key
    }

    /// Attaches worker pre-packed GPU vertices and optional `u16` indices.
    pub fn with_prepacked_gpu(mut self, prepacked: PrepackedGpuGeometry) -> Self {
        let len = logical_count_capped(prepacked.logical_len());
        self.uploaded_vertices = self.uploaded_vertices.max(len);
        self.prepacked_gpu = Some(Arc::new(prepacked));
        self
    }

    /// Returns read-only slices into attached pre-packed GPU geometry when present.
    #[inline]
    pub fn prepacked_slices(&self) -> Option<PrepackedSlices<'_>> {
        let pre = self.prepacked_gpu.as_deref()?;
        Some(PrepackedSlices {
            vertices: &pre.vertices,
            indices_u16: pre.indices_u16.as_deref(),
            indices_u32: pre.indices_u32.as_deref(),
            compact_extrusion: pre.compact_extrusion,
            logical_len: pre.logical_len(),
        })
    }

    /// Consumes this buffer and returns its owned raw storage and metadata.
    pub fn into_raw_parts(self) -> RawVertexBufferParts<V, I> {
        RawVertexBufferParts {
            vertices: self.buffer.vertices,
            indices: self.buffer.indices,
            usable_indices: self.usable_indices,
            shared_geometry_key: self.shared_geometry_key,
            prepacked_gpu: self.prepacked_gpu,
        }
    }

    /// Returns worker pre-packed GPU geometry when present.
    #[inline]
    pub fn prepacked_gpu(&self) -> Option<&PrepackedGpuGeometry> {
        self.prepacked_gpu.as_deref()
    }

    /// Number of index elements currently stored in CPU memory (`u16` prepacked, `u32` prepacked, or raw).
    #[inline]
    pub fn cpu_index_len(&self) -> usize {
        let fallback = self.buffer.indices.len();
        self.prepacked_gpu
            .as_ref()
            .map_or(fallback, |p| p.index_len(fallback))
    }

    /// Byte capacity of pre-packed GPU buffers currently held in CPU memory.
    #[inline]
    pub fn prepacked_bytes(&self) -> usize {
        self.prepacked_gpu.as_ref().map_or(0, |p| p.byte_capacity())
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
            || self.prepacked_gpu.as_ref().is_some_and(|p| p.has_indices())
    }

    /// Returns `true` when this buffer carries non-empty drawable geometry.
    #[inline]
    pub fn has_drawable_geometry(&self) -> bool {
        self.usable_indices > 0 || self.has_cpu_indices()
    }

    /// Releases CPU-side geometry after GPU upload while preserving `vertex_count()`.
    pub fn release_cpu_geometry(&mut self) {
        let count = logical_count_capped(self.vertex_count());
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
        icon_bits_storage_bytes(self.icon_bits.capacity())
    }
}

impl<V, I> std::ops::Deref for OverAlignedVertexBuffer<V, I> {
    type Target = VertexBuffers<V, I>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.buffer
    }
}

impl<V, I> std::ops::DerefMut for OverAlignedVertexBuffer<V, I> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.buffer
    }
}

impl<I> OverAlignedVertexBuffer<symbol_vertex::ShaderSymbolVertex, I> {
    pub(crate) fn is_icon_vertex(&self, index: usize) -> bool {
        if let Some(vertex) = self.buffer.vertices.get(index) {
            matches!(vertex.kind(), 1 | 2)
        } else {
            test_icon_bit(&self.icon_bits, index)
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
                set_icon_bit(&mut bits, i);
            }
        }
        self.uploaded_vertices = logical_count_capped(count);
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
        while !is_vertex_buffer_aligned(self.vertices.len(), stride, align) {
            self.vertices.push(V::zeroed());
        }
    }

    fn align_indices(&mut self) {
        let align = wgpu::COPY_BUFFER_ALIGNMENT;
        let stride = std::mem::size_of::<I>() as wgpu::BufferAddress;
        let overpad = calc_index_overpad_count(self.indices.len(), stride, align);
        for _ in 0..overpad {
            self.indices.push(I::zeroed());
        }
    }
}

#[inline]
fn resolve_logical_vertex_count(logical_vertex_count: u32, raw_len: usize) -> usize {
    if logical_vertex_count > 0 {
        logical_vertex_count as usize
    } else {
        raw_len
    }
}

#[inline]
fn calc_prepacked_storage_bytes(verts_cap: usize, u16_cap: usize, u32_cap: usize) -> usize {
    verts_cap * std::mem::size_of::<gpu_vertex::GpuShaderVertex>()
        + u16_cap * std::mem::size_of::<u16>()
        + u32_cap * std::mem::size_of::<u32>()
}

#[inline]
fn logical_count_capped(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

#[inline]
fn icon_bits_storage_bytes(bits_cap: usize) -> usize {
    bits_cap * std::mem::size_of::<u64>()
}

#[inline]
fn test_icon_bit(bits: &[u64], index: usize) -> bool {
    bits.get(index / 64)
        .is_some_and(|word| ((word >> (index % 64)) & 1) != 0)
}

#[inline]
fn set_icon_bit(bits: &mut [u64], index: usize) {
    if let Some(word) = bits.get_mut(index / 64) {
        *word |= 1_u64 << (index % 64);
    }
}

#[inline]
fn is_vertex_buffer_aligned(len: usize, stride: u64, align: u64) -> bool {
    (len as u64 * stride).is_multiple_of(align)
}

#[inline]
fn calc_index_overpad_count(len: usize, stride: u64, align: u64) -> usize {
    let unpadded_bytes = len as u64 * stride;
    let padding_bytes = (align - unpadded_bytes % align) % align;
    padding_bytes.div_ceil(stride) as usize
}
