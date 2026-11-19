use super::*;

impl<Q: Queue<B>, B, V: PoolVertex, I: Pod, TM: Pod, FM: Pod> BufferPool<Q, B, V, I, TM, FM> {
    /// Plans whether `indices` can be uploaded into a `u16` slab (`IndexFormat::Uint16`, either
    /// directly or via `BASE_VERTEX` sub-chunks) and returns `(index_u16, copy_aligned_index_bytes, u16_chunks)`.
    pub(super) fn plan_index_upload(
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
                if u32_slice[..usable]
                    .iter()
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
                    .extend(u32_slice[..usable].iter().map(|&idx| idx as u16));
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
            if self.u16_index_scratch.capacity() > 98_304 {
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
        let prepacked_info = Self::prepacked_info(geometry);
        let prepacked_verts = prepacked_info.map(|(v, _, cmp)| (v, cmp));
        let strides = ElementStrides::of::<V::Gpu, I, TM, FM>();
        let (vertices, records) = (
            prepacked_info.map_or(geometry.buffer.vertices.len(), |(_, len, _)| len) as u64,
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

        let allow_chunks = u16_chunks::layer_allows_u16_chunks(&style_layer);
        let (index_u16, index_bytes, u16_chunk_ranges) =
            self.plan_geometry_index_upload(geometry, vertices, strides, allow_chunks);
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
        let dynamic_circle_fm = dual_fm
            && matches!(
                style_layer.paint.as_ref(),
                Some(crate::style::layer::LayerPaint::Circle(paint))
                    if paint.stroke_color_per_feature().is_none()
            );
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
            dynamic_circle_metadata: dynamic_circle_fm,
            shared_geometry_key: None,
            evaluated_zoom: None,
            u16_chunks: u16_chunk_ranges,
            compact_extrusion: false,
        };

        if req.vertex_bytes > 0 {
            self.write_geometry_vertices(
                queue,
                &entry,
                prepacked_verts,
                vertices as usize,
                &geometry.buffer.vertices,
            );
        }
        self.write_geometry_indices(queue, &entry, geometry, req.index_bytes);
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
pub(super) fn element_index(index: u64) -> Result<u32, AllocationError> {
    u32::try_from(index).map_err(|_| AllocationError::Capacity {
        buffer: BackingBufferType::Vertices,
        requested: index,
        capacity: u64::from(u32::MAX),
    })
}
