use super::*;

impl<Q: Queue<B>, B, V: PoolVertex, I: Pod, TM: Pod, FM: Pod> BufferPool<Q, B, V, I, TM, FM> {
    /// Plans whether `indices` can be uploaded into a `u16` slab (`IndexFormat::Uint16`) and
    /// returns `(index_u16, copy_aligned_index_bytes)`.
    pub(super) fn plan_index_upload(
        &self,
        indices: &[I],
        usable_indices: u32,
        vertex_count: u64,
        strides: ElementStrides,
    ) -> (bool, u64) {
        if self.enable_u16_indices && size_of::<I>() == size_of::<u32>() && vertex_count <= 65_536 {
            let u32_slice: &[u32] = bytemuck::cast_slice(indices);
            let usable = (usable_indices as usize).min(u32_slice.len());
            if u32_slice[..usable]
                .iter()
                .all(|&idx| idx <= u32::from(u16::MAX))
            {
                let padded_u16 = usable.next_multiple_of(2);
                return (true, (padded_u16 as u64) * 2);
            }
        }
        (false, (indices.len() as u64) * strides.index)
    }

    /// Writes `indices` (`u16`-downcast or native `I`) into `entry`'s reserved index slab.
    pub(super) fn write_uploaded_indices(
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
            let padded = usable.next_multiple_of(2);
            self.u16_index_scratch.clear();
            self.u16_index_scratch.reserve(padded);
            self.u16_index_scratch
                .extend(u32_slice[..usable].iter().map(|&idx| idx as u16));
            if self.u16_index_scratch.len() < padded {
                self.u16_index_scratch.push(0);
            }
            let buf = &self.index_u16_slab_of(entry.index_page).buffer.inner;
            queue.write_buffer(
                buf,
                entry.buffer_indices.start,
                bytemuck::cast_slice(&self.u16_index_scratch),
            );
        } else {
            let buf = &self.index_native_slab_of(entry.index_page).buffer.inner;
            queue.write_buffer(
                buf,
                entry.buffer_indices.start,
                bytemuck::cast_slice(indices),
            );
        }
    }

    /// Uploads a layer atomically, growing slabs or evicting older allocations when required.
    pub fn allocate_layer_geometry(
        &mut self,
        queue: &Q,
        coords: WorldTileCoords,
        style_layer: StyleLayer,
        geometry: &OverAlignedVertexBuffer<V, I>,
        layer_metadata: TM,
        feature_metadata: &[FM],
    ) -> Result<(), AllocationError> {
        self.allocate_layer_geometry_unless(
            queue,
            coords,
            style_layer,
            geometry,
            layer_metadata,
            feature_metadata,
            |_| false,
        )
    }

    /// Uploads a layer atomically, refusing to evict allocations of tiles matching `protect`.
    #[allow(clippy::too_many_arguments)]
    pub fn allocate_layer_geometry_unless(
        &mut self,
        queue: &Q,
        coords: WorldTileCoords,
        style_layer: StyleLayer,
        geometry: &OverAlignedVertexBuffer<V, I>,
        layer_metadata: TM,
        feature_metadata: &[FM],
        mut protect: impl FnMut(WorldTileCoords) -> bool,
    ) -> Result<(), AllocationError> {
        if coords.build_quad_key().is_none() {
            return Err(AllocationError::Coordinates { coords });
        }
        let strides = ElementStrides::of::<V::Gpu, I, TM, FM>();
        let (vertices, records) = (
            geometry.buffer.vertices.len() as u64,
            feature_metadata.len() as u64,
        );
        let dual_fm = self.decoupled_feature_metadata && vertices > 0 && records == 2 * vertices;
        if records > vertices && !dual_fm {
            return Err(AllocationError::FeatureMetadata { vertices, records });
        }

        let fm_kind = if strides.feature_metadata == 0 {
            FmStorageKind::None
        } else if !self.decoupled_feature_metadata {
            FmStorageKind::Paired
        } else if records == 0 {
            FmStorageKind::None
        } else if dual_fm || self.page_factory.is_none() {
            FmStorageKind::Standalone
        } else {
            FmStorageKind::Paired
        };

        let (index_u16, index_bytes) = self.plan_index_upload(
            &geometry.buffer.indices,
            geometry.usable_indices,
            vertices,
            strides,
        );
        let req = SlabRequest {
            vertex_bytes: vertices * strides.vertex,
            vertex_elem_align: V::vertex_element_alignment(&style_layer.type_),
            index_bytes,
            index_u16,
            layer_bytes: strides.layer_metadata,
            feature_bytes: records * strides.feature_metadata,
            fm_kind,
        };
        self.validate_request(req, strides)?;

        let style_layer_id = style_layer.layer_id();
        let family = V::encoding_family(&style_layer.type_);
        let layer_meta_key = self.layer_meta_share_key(style_layer_id, coords);
        let alloc = self.reserve_allocation(
            Some(queue),
            req,
            strides,
            layer_meta_key,
            None,
            &mut protect,
        )?;
        let idx_stride = strides.active_index_stride(alloc.index_u16);
        let first_vertex = alloc
            .vertices
            .start
            .checked_div(strides.vertex)
            .unwrap_or(0);
        let first_index = alloc.indices.start.checked_div(idx_stride).unwrap_or(0);
        let (first_vertex_u32, first_index_u32) =
            match (element_index(first_vertex), element_index(first_index)) {
                (Ok(fv), Ok(fi)) => (fv, fi),
                (Err(err), _) | (_, Err(err)) => {
                    self.abort_allocation(alloc, true, layer_meta_key);
                    return Err(err);
                }
            };

        let allocation_id = self.revision.wrapping_add(1);
        let entry = IndexEntry {
            allocation_id,
            paint_revision: 0,
            vertex_page: alloc.vertex_page,
            index_page: alloc.index_page,
            layer_page: alloc.layer_page,
            feature_page: alloc.feature_page,
            index_u16: alloc.index_u16,
            fm_kind: alloc.fm_kind,
            shared_vertex_family: family,
            coords,
            style_layer_id,
            style_layer,
            usable_indices: geometry.usable_indices,
            first_vertex: first_vertex_u32,
            first_index: first_index_u32,
            buffer_vertices: alloc.vertices,
            buffer_indices: alloc.indices,
            buffer_layer_metadata: alloc.layer_metadata,
            buffer_feature_metadata: alloc.feature_metadata,
            uniform_feature_metadata: false,
            dual_feature_metadata: dual_fm,
            shared_geometry_key: None,
        };

        if req.vertex_bytes > 0 {
            let mut scratch = std::mem::take(&mut self.vertex_scratch);
            let gpu_verts = V::encode_vertices(
                &entry.style_layer.type_,
                &geometry.buffer.vertices,
                &mut scratch,
            );
            queue.write_buffer(
                &self.vertex_slab_of(entry.vertex_page).vertices.inner,
                entry.buffer_vertices.start,
                bytemuck::cast_slice(gpu_verts),
            );
            self.vertex_scratch = scratch;
        }
        self.write_uploaded_indices(
            queue,
            &entry,
            &geometry.buffer.indices,
            geometry.usable_indices,
            req.index_bytes,
        );
        if req.layer_bytes > 0 && entry.layer_page != NO_SLAB {
            queue.write_buffer(
                self.metadata_for(&entry),
                entry.buffer_layer_metadata.start,
                bytemuck::bytes_of(&layer_metadata),
            );
        }
        if req.feature_bytes > 0 {
            if entry.fm_kind == FmStorageKind::Paired {
                if let Some(v_slab) = self
                    .vertex_slabs
                    .get_mut(usize::from(entry.vertex_page))
                    .and_then(Option::as_mut)
                {
                    v_slab.paired_fm_dirty_end = v_slab
                        .paired_fm_dirty_end
                        .max(entry.buffer_feature_metadata.end);
                }
            }
            queue.write_buffer(
                self.feature_metadata_for(&entry),
                entry.buffer_feature_metadata.start,
                bytemuck::cast_slice(feature_metadata),
            );
        }

        self.index.push_back(entry);
        self.revision = allocation_id;
        Ok(())
    }

    /// Writes `count` copies of `value` into `buffer` starting at `offset` using fixed stack chunks.
    pub(super) fn write_repeated_feature_metadata(
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
            queue.write_buffer(buffer, offset, bytemuck::cast_slice(&chunk[..n]));
            offset += (n as wgpu::BufferAddress) * stride;
            count -= n;
        }
    }

    /// Replaces only this style layer with uniform feature metadata without heap scratch.
    #[allow(clippy::too_many_arguments)]
    pub fn replace_layer_geometry_uniform_unless(
        &mut self,
        queue: &Q,
        coords: WorldTileCoords,
        style_layer: StyleLayer,
        geometry: &OverAlignedVertexBuffer<V, I>,
        layer_metadata: TM,
        uniform_feature: FM,
        records: usize,
        protect: impl FnMut(WorldTileCoords) -> bool,
    ) -> Result<(), AllocationError> {
        self.replace_layer_geometry_uniform_shared_unless(
            queue,
            coords,
            style_layer,
            geometry,
            None,
            layer_metadata,
            uniform_feature,
            records,
            protect,
        )
    }

    /// Replaces only this style layer once the new geometry has been validated and uploaded.
    pub fn replace_layer_geometry(
        &mut self,
        queue: &Q,
        coords: WorldTileCoords,
        style_layer: StyleLayer,
        geometry: &OverAlignedVertexBuffer<V, I>,
        layer_metadata: TM,
        feature_metadata: &[FM],
    ) -> Result<(), AllocationError> {
        self.replace_layer_geometry_unless(
            queue,
            coords,
            style_layer,
            geometry,
            layer_metadata,
            feature_metadata,
            |_| false,
        )
    }

    /// Replaces only this style layer while refusing to evict tiles matching `protect`.
    #[allow(clippy::too_many_arguments)]
    pub fn replace_layer_geometry_unless(
        &mut self,
        queue: &Q,
        coords: WorldTileCoords,
        style_layer: StyleLayer,
        geometry: &OverAlignedVertexBuffer<V, I>,
        layer_metadata: TM,
        feature_metadata: &[FM],
        protect: impl FnMut(WorldTileCoords) -> bool,
    ) -> Result<(), AllocationError> {
        let style_layer_id = style_layer.layer_id();
        self.allocate_layer_geometry_unless(
            queue,
            coords,
            style_layer,
            geometry,
            layer_metadata,
            feature_metadata,
            protect,
        )?;
        let removed = self
            .index
            .remove_layer(coords, style_layer_id, Some(self.revision));
        for entry in &removed {
            self.free_entry_ranges(entry);
        }
        if !removed.is_empty() {
            self.reclaim_empty_pages_with_queue(1, Some(queue));
        }
        Ok(())
    }

    pub(super) fn free_entry_ranges(&mut self, entry: &IndexEntry) {
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

    pub(super) fn reserve_allocation(
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
pub(super) fn element_index(index: u64) -> Result<u32, AllocationError> {
    u32::try_from(index).map_err(|_| AllocationError::Capacity {
        buffer: BackingBufferType::Vertices,
        requested: index,
        capacity: u64::from(u32::MAX),
    })
}
