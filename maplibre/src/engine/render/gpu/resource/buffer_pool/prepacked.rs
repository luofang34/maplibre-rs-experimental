//! Zero-conversion GPU uploads for worker pre-packed
//! [`PrepackedGpuGeometry`](crate::render::resource::vertex_buffer::PrepackedGpuGeometry) buffers.

use std::any::TypeId;

use super::*;
use crate::render::resource::vertex_buffer::gpu_vertex::{
    unpack_compact_extrusion_vertices, GpuShaderVertex,
};

impl<Q: Queue<B>, B, V: PoolVertex, I: Pod, TM: Pod, FM: Pod> BufferPool<Q, B, V, I, TM, FM> {
    /// Returns `(prepacked_words, logical_vertex_count, compact_extrusion)` when `V::Gpu` matches
    /// [`GpuShaderVertex`].
    #[inline]
    pub(super) fn prepacked_info(
        geometry: &OverAlignedVertexBuffer<V, I>,
    ) -> Option<(&[GpuShaderVertex], usize, bool)> {
        if TypeId::of::<V::Gpu>() != TypeId::of::<GpuShaderVertex>() {
            return None;
        }
        let prepacked = geometry.prepacked_slices()?;
        (!prepacked.vertices.is_empty()).then_some((
            prepacked.vertices,
            prepacked.logical_len,
            prepacked.compact_extrusion,
        ))
    }

    /// Plans `(index_u16, copy_aligned_index_bytes, u16_chunks)` honoring worker pre-narrowed `u16`
    /// or pre-packed `u32` indices.
    pub(super) fn plan_geometry_index_upload(
        &self,
        geometry: &OverAlignedVertexBuffer<V, I>,
        vertex_count: u64,
        strides: ElementStrides,
        allow_chunks: bool,
    ) -> u16_chunks::IndexPlan {
        if let Some(pre) = geometry.prepacked_slices() {
            if TypeId::of::<I>() == TypeId::of::<u32>() {
                if let Some(u16s) = pre.indices_u16 {
                    if self.enable_u16_indices {
                        let usable = (geometry.usable_indices as usize).min(u16s.len());
                        let padded = usable.next_multiple_of(2);
                        return (true, (padded as u64) * 2, None);
                    }
                    let usable = (geometry.usable_indices as usize).min(u16s.len());
                    return (false, (usable as u64) * strides.index, None);
                }
                if let Some(u32s) = pre.indices_u32 {
                    return self.plan_index_upload(
                        bytemuck::cast_slice(u32s),
                        geometry.usable_indices,
                        vertex_count,
                        strides,
                        allow_chunks,
                    );
                }
            }
        }
        self.plan_index_upload(
            &geometry.indices,
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
        prepacked_verts: Option<(&[GpuShaderVertex], bool)>,
        logical_vertices: usize,
        source_vertices: &[V],
    ) {
        if let Some((pre, is_compact)) = prepacked_verts {
            if is_compact == entry.compact_extrusion {
                queue.write_buffer(
                    &self.vertex_slab_of(entry.vertex_page).vertices.inner,
                    entry.buffer_vertices.start,
                    bytemuck::cast_slice(pre),
                );
                return;
            }
            if is_compact && !entry.compact_extrusion {
                let mut unpacked = Vec::new();
                unpack_compact_extrusion_vertices(pre, logical_vertices, &mut unpacked);
                queue.write_buffer(
                    &self.vertex_slab_of(entry.vertex_page).vertices.inner,
                    entry.buffer_vertices.start,
                    bytemuck::cast_slice(&unpacked),
                );
                return;
            }
        }
        let mut scratch = std::mem::take(&mut self.vertex_scratch);
        let bytes_to_write: &[u8] = if entry.compact_extrusion {
            if let Some(compact) = V::try_encode_compact_extrusion(
                &entry.style_layer.type_,
                source_vertices,
                &mut scratch,
            ) {
                bytemuck::cast_slice(compact)
            } else {
                bytemuck::cast_slice(V::encode_vertices(
                    &entry.style_layer.type_,
                    source_vertices,
                    &mut scratch,
                ))
            }
        } else {
            bytemuck::cast_slice(V::encode_vertices(
                &entry.style_layer.type_,
                source_vertices,
                &mut scratch,
            ))
        };
        queue.write_buffer(
            &self.vertex_slab_of(entry.vertex_page).vertices.inner,
            entry.buffer_vertices.start,
            bytes_to_write,
        );
        if scratch.capacity() > 4_096 {
            scratch.clear();
            scratch.shrink_to(0);
        }
        self.vertex_scratch = scratch;
    }

    /// Writes `indices` directly from worker pre-narrowed `u16` or pre-packed `u32` indices when present.
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
        if let Some(pre) = geometry.prepacked_slices() {
            if TypeId::of::<I>() == TypeId::of::<u32>() {
                if let Some(u16s) = pre.indices_u16 {
                    let usable = (geometry.usable_indices as usize).min(u16s.len());
                    let usable_u16s = u16s.get(..usable).unwrap_or(u16s);
                    if entry.index_u16 {
                        let padded = usable.next_multiple_of(2);
                        if let Some(padded_slice) = (padded * 2 == index_bytes as usize)
                            .then(|| u16s.get(..padded))
                            .flatten()
                        {
                            queue.write_buffer(
                                self.indices_for(entry),
                                entry.buffer_indices.start,
                                bytemuck::cast_slice(padded_slice),
                            );
                        } else {
                            self.u16_index_scratch.clear();
                            self.u16_index_scratch.reserve(padded);
                            self.u16_index_scratch.extend_from_slice(usable_u16s);
                            if self.u16_index_scratch.len() < padded {
                                self.u16_index_scratch.push(0);
                            }
                            queue.write_buffer(
                                self.indices_for(entry),
                                entry.buffer_indices.start,
                                bytemuck::cast_slice(&self.u16_index_scratch),
                            );
                            self.u16_index_scratch.clear();
                            if self.u16_index_scratch.capacity() > 8_192 {
                                self.u16_index_scratch.shrink_to(8_192);
                            }
                        }
                    } else {
                        let wide: Vec<u32> = usable_u16s.iter().copied().map(u32::from).collect();
                        queue.write_buffer(
                            self.indices_for(entry),
                            entry.buffer_indices.start,
                            bytemuck::cast_slice(&wide),
                        );
                    }
                    return;
                }
                if let Some(u32s) = pre.indices_u32 {
                    self.write_uploaded_indices(
                        queue,
                        entry,
                        bytemuck::cast_slice(u32s),
                        geometry.usable_indices,
                        index_bytes,
                    );
                    return;
                }
            }
        }
        self.write_uploaded_indices(
            queue,
            entry,
            &geometry.indices,
            geometry.usable_indices,
            index_bytes,
        );
    }

    /// Plans whether `indices` can be uploaded into a `u16` slab (`IndexFormat::Uint16`, either
    /// directly or via `BASE_VERTEX` sub-chunks) and returns `(index_u16, copy_aligned_index_bytes, u16_chunks)`.
    pub(in crate::engine::render::gpu::resource::buffer_pool) fn plan_index_upload(
        &self,
        indices: &[I],
        usable_indices: u32,
        vertex_count: u64,
        strides: ElementStrides,
        allow_chunks: bool,
    ) -> u16_chunks::IndexPlan {
        if self.enable_u16_indices && size_of::<I>() == size_of::<u32>() {
            let u32_slice: &[u32] = bytemuck::cast_slice(indices);
            let usable = (usable_indices as usize).min(u32_slice.len());
            if vertex_count <= 65_536 {
                if u32_slice
                    .iter()
                    .take(usable)
                    .all(|&idx| idx <= u32::from(u16::MAX))
                {
                    let padded_u16 = usable.next_multiple_of(2);
                    return (true, (padded_u16 as u64) * 2, None);
                }
            } else if self.enable_u16_chunks && allow_chunks && usable > 0 {
                let padded_u16 = usable.next_multiple_of(2);
                let u16_bytes = (padded_u16 as u64) * 2;
                if u16_bytes <= self.max_page_bytes {
                    if let Some(chunks) = u16_chunks::plan_u16_chunks(u32_slice, usable) {
                        return (true, u16_bytes, Some(chunks));
                    }
                }
            }
        }
        (false, (indices.len() as u64) * strides.index, None)
    }

    /// Writes `indices` (`u16`-downcast, chunked `u16`, or native `I`) into `entry`'s reserved index slab.
    pub(in crate::engine::render::gpu::resource::buffer_pool) fn write_uploaded_indices(
        &mut self,
        queue: &Q,
        entry: &IndexEntry,
        indices: &[I],
        usable_indices: u32,
        index_bytes: u64,
    ) {
        if index_bytes == 0 || entry.index_page == NO_SLAB {
            return;
        }
        if entry.index_u16 {
            let u32_slice: &[u32] = bytemuck::cast_slice(indices);
            let usable = (usable_indices as usize).min(u32_slice.len());
            if let Some(chunks) = entry.u16_chunks() {
                u16_chunks::write_chunked_u16_indices(
                    u32_slice,
                    usable,
                    chunks,
                    &mut self.u16_index_scratch,
                );
            } else {
                let padded = usable.next_multiple_of(2);
                self.u16_index_scratch.clear();
                self.u16_index_scratch.reserve(padded);
                self.u16_index_scratch
                    .extend(u32_slice.iter().take(usable).map(|&idx| idx as u16));
                if self.u16_index_scratch.len() < padded {
                    self.u16_index_scratch.push(0);
                }
            }
            let buf = &self.index_u16_slab_of(entry.index_page).buffer.inner;
            queue.write_buffer(
                buf,
                entry.buffer_indices.start,
                bytemuck::cast_slice(&self.u16_index_scratch),
            );
            if self.u16_index_scratch.capacity() > 8_192 {
                self.u16_index_scratch.clear();
                self.u16_index_scratch.shrink_to(0);
            }
        } else {
            let buf = &self.index_native_slab_of(entry.index_page).buffer.inner;
            queue.write_buffer(
                buf,
                entry.buffer_indices.start,
                bytemuck::cast_slice(indices),
            );
        }
    }

    /// Writes `count` copies of `value` into `buffer` starting at `offset` using fixed stack chunks.
    pub(in crate::engine::render::gpu::resource::buffer_pool) fn write_repeated_feature_metadata(
        queue: &Q,
        buffer: &B,
        mut offset: wgpu::BufferAddress,
        value: FM,
        mut count: usize,
    ) {
        let stride = size_of::<FM>() as wgpu::BufferAddress;
        if stride == 0 || count == 0 {
            return;
        }
        let chunk = [value; 1024];
        while count > 0 {
            let n = count.min(chunk.len());
            if let Some(slice) = chunk.get(..n) {
                queue.write_buffer(buffer, offset, bytemuck::cast_slice(slice));
            }
            offset += (n as wgpu::BufferAddress) * stride;
            count -= n;
        }
    }
}

/// An element index as the 32 bits a draw call takes; a slab beyond them is not drawable.
pub(in crate::engine::render::gpu::resource::buffer_pool) fn element_index(
    index: u64,
) -> Result<u32, AllocationError> {
    u32::try_from(index).map_err(|_| AllocationError::Capacity {
        buffer: BackingBufferType::Vertices,
        requested: index,
        capacity: u64::from(u32::MAX),
    })
}
