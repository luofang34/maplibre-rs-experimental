use super::*;

impl<Q: Queue<B>, B, V: PoolVertex, I: Pod, TM: Pod, FM: Pod> BufferPool<Q, B, V, I, TM, FM> {
    /// Plans whether `indices` can be uploaded into a `u16` slab (`IndexFormat::Uint16`, either
    /// directly or via `BASE_VERTEX` sub-chunks) and returns `(index_u16, copy_aligned_index_bytes, u16_chunks)`.
    pub(in crate::vector::resource::buffer_pool) fn plan_index_upload(
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
    pub(in crate::vector::resource::buffer_pool) fn write_uploaded_indices(
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
    pub(in crate::vector::resource::buffer_pool) fn write_repeated_feature_metadata(
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

    pub(in crate::vector::resource::buffer_pool) fn free_entry_ranges(
        &mut self,
        entry: &IndexEntry,
    ) {
        let release_vertices = self.release_shared_vertex_ref(
            entry.coords,
            entry.vertex_page,
            entry.shared_geometry_key,
            entry.shared_vertex_family,
        );
        if entry.vertex_page != NO_SLAB {
            if let Some(slab) = self
                .vertex_slabs
                .get_mut(usize::from(entry.vertex_page))
                .and_then(Option::as_mut)
            {
                if release_vertices {
                    slab.vertex_alloc.free(entry.buffer_vertices.clone());
                }
                slab.live_entries = slab.live_entries.saturating_sub(1);
            }
        }
        if entry.index_page != NO_SLAB {
            let slabs = if entry.index_u16 {
                &mut self.index_u16_slabs
            } else {
                &mut self.index_native_slabs
            };
            if let Some(slab) = slabs
                .get_mut(usize::from(entry.index_page))
                .and_then(Option::as_mut)
            {
                slab.alloc.free(entry.buffer_indices.clone());
                slab.live_entries = slab.live_entries.saturating_sub(1);
            }
        }
        if entry.layer_page != NO_SLAB {
            let release_layer_meta = self.release_shared_layer_meta_ref(
                entry.layer_page,
                entry.style_layer_id,
                u8::from(entry.coords.z),
            );
            if let Some(slab) = self
                .layer_slabs
                .get_mut(usize::from(entry.layer_page))
                .and_then(Option::as_mut)
            {
                if release_layer_meta {
                    slab.alloc.free(entry.buffer_layer_metadata.clone());
                }
                slab.live_entries = slab.live_entries.saturating_sub(1);
            }
        }
        if entry.fm_kind == FmStorageKind::Standalone && entry.feature_page != NO_SLAB {
            if let Some(slab) = self
                .feature_slabs
                .get_mut(usize::from(entry.feature_page))
                .and_then(Option::as_mut)
            {
                slab.alloc.free(entry.buffer_feature_metadata.clone());
                slab.live_entries = slab.live_entries.saturating_sub(1);
            }
        }
    }

    pub(in crate::vector::resource::buffer_pool) fn reserve_allocation(
        &mut self,
        queue: Option<&Q>,
        req: SlabRequest,
        strides: ElementStrides,
        layer_meta_key: Option<(crate::style::StyleLayerId, u8)>,
        shared_vertex_slot: Option<(u16, Range<u64>)>,
        mut protect: impl FnMut(WorldTileCoords) -> bool,
    ) -> Result<SlabAllocation, AllocationError> {
        if let Some(alloc) = self.try_reserve_slabs(
            queue,
            req,
            strides,
            layer_meta_key,
            shared_vertex_slot.clone(),
        ) {
            return Ok(alloc);
        }

        let mut min_alloc_id = 0_u64;
        while let Some(evicted) = self.index.pop_oldest_from(&mut min_alloc_id, &mut protect) {
            self.pattern_runs
                .remove(&(evicted.coords, evicted.style_layer_id));
            self.sort_runs
                .remove(&(evicted.coords, evicted.style_layer_id));
            self.free_entry_ranges(&evicted);
            self.eviction_generation = self.eviction_generation.wrapping_add(1);

            if let Some(alloc) = self.try_reserve_slabs(
                queue,
                req,
                strides,
                layer_meta_key,
                shared_vertex_slot.clone(),
            ) {
                return Ok(alloc);
            }
        }

        Err(self.saturation_error(req, strides))
    }
}

/// An element index as the 32 bits a draw call takes; a slab beyond them is not drawable.
pub(in crate::vector::resource::buffer_pool) fn element_index(
    index: u64,
) -> Result<u32, AllocationError> {
    u32::try_from(index).map_err(|_| AllocationError::Capacity {
        buffer: BackingBufferType::Vertices,
        requested: index,
        capacity: u64::from(u32::MAX),
    })
}
