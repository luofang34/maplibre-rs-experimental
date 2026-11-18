//! Zero-conversion GPU uploads for worker pre-packed
//! [`PrepackedGpuGeometry`](crate::vector::tessellation::PrepackedGpuGeometry) buffers.

use std::any::TypeId;

use super::*;
use crate::render::shaders::GpuShaderVertex;

impl<Q: Queue<B>, B, V: PoolVertex, I: Pod, TM: Pod, FM: Pod> BufferPool<Q, B, V, I, TM, FM> {
    /// Returns pre-packed 16-byte GPU vertices when attached and `V::Gpu` matches
    /// [`GpuShaderVertex`].
    #[inline]
    pub(super) fn prepacked_vertices(
        geometry: &OverAlignedVertexBuffer<V, I>,
    ) -> Option<&[GpuShaderVertex]> {
        if TypeId::of::<V::Gpu>() != TypeId::of::<GpuShaderVertex>() {
            return None;
        }
        let prepacked = geometry.prepacked_gpu()?;
        (!prepacked.vertices.is_empty()).then_some(prepacked.vertices.as_slice())
    }

    /// Plans `(index_u16, copy_aligned_index_bytes, u16_chunks)` honoring worker pre-narrowed `u16` indices.
    pub(super) fn plan_geometry_index_upload(
        &self,
        geometry: &OverAlignedVertexBuffer<V, I>,
        vertex_count: u64,
        strides: ElementStrides,
        allow_chunks: bool,
    ) -> u16_chunks::IndexPlan {
        if let Some(u16s) = geometry
            .prepacked_gpu()
            .and_then(|p| p.indices_u16.as_deref())
        {
            if TypeId::of::<I>() == TypeId::of::<u32>() {
                if self.enable_u16_indices {
                    let usable = (geometry.usable_indices as usize).min(u16s.len());
                    let padded = usable.next_multiple_of(2);
                    return (true, (padded as u64) * 2, None);
                }
                let usable = (geometry.usable_indices as usize).min(u16s.len());
                return (false, (usable as u64) * strides.index, None);
            }
        }
        self.plan_index_upload(
            &geometry.buffer.indices,
            geometry.usable_indices,
            vertex_count,
            strides,
            allow_chunks,
        )
    }

    /// Writes encoded GPU vertices directly from worker pre-packed storage or via `V::encode_vertices`.
    pub(super) fn write_geometry_vertices(
        &mut self,
        queue: &Q,
        entry: &IndexEntry,
        prepacked_verts: Option<&[GpuShaderVertex]>,
        source_vertices: &[V],
    ) {
        if let Some(pre) = prepacked_verts {
            queue.write_buffer(
                &self.vertex_slab_of(entry.vertex_page).vertices.inner,
                entry.buffer_vertices.start,
                bytemuck::cast_slice(pre),
            );
            return;
        }
        let mut scratch = std::mem::take(&mut self.vertex_scratch);
        let gpu_verts = V::encode_vertices(&entry.style_layer.type_, source_vertices, &mut scratch);
        queue.write_buffer(
            &self.vertex_slab_of(entry.vertex_page).vertices.inner,
            entry.buffer_vertices.start,
            bytemuck::cast_slice(gpu_verts),
        );
        #[cfg(target_arch = "wasm32")]
        if scratch.capacity() > 65_536 {
            scratch = Vec::new();
        }
        self.vertex_scratch = scratch;
    }

    /// Writes `indices` directly from worker pre-narrowed `u16` indices when present.
    pub(super) fn write_geometry_indices(
        &mut self,
        queue: &Q,
        entry: &IndexEntry,
        geometry: &OverAlignedVertexBuffer<V, I>,
        index_bytes: u64,
    ) {
        if index_bytes == 0 || entry.index_page == NO_SLAB {
            return;
        }
        if let Some(u16s) = geometry
            .prepacked_gpu()
            .and_then(|p| p.indices_u16.as_deref())
        {
            if TypeId::of::<I>() == TypeId::of::<u32>() {
                let usable = (geometry.usable_indices as usize).min(u16s.len());
                if entry.index_u16 {
                    let padded = usable.next_multiple_of(2);
                    if u16s.len() >= padded && padded * 2 == index_bytes as usize {
                        queue.write_buffer(
                            self.indices_for(entry),
                            entry.buffer_indices.start,
                            bytemuck::cast_slice(&u16s[..padded]),
                        );
                    } else {
                        self.u16_index_scratch.clear();
                        self.u16_index_scratch.reserve(padded);
                        self.u16_index_scratch.extend_from_slice(&u16s[..usable]);
                        if self.u16_index_scratch.len() < padded {
                            self.u16_index_scratch.push(0);
                        }
                        queue.write_buffer(
                            self.indices_for(entry),
                            entry.buffer_indices.start,
                            bytemuck::cast_slice(&self.u16_index_scratch),
                        );
                        self.u16_index_scratch.clear();
                    }
                } else {
                    let wide: Vec<u32> = u16s[..usable].iter().copied().map(u32::from).collect();
                    queue.write_buffer(
                        self.indices_for(entry),
                        entry.buffer_indices.start,
                        bytemuck::cast_slice(&wide),
                    );
                }
                return;
            }
        }
        self.write_uploaded_indices(
            queue,
            entry,
            &geometry.buffer.indices,
            geometry.usable_indices,
            index_bytes,
        );
    }
}
