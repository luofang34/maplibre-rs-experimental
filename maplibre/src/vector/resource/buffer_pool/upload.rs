use super::*;

impl<Q: Queue<B>, B, V: Pod, I: Pod, TM: Pod, FM: Pod> BufferPool<Q, B, V, I, TM, FM> {
    /// Uploads a layer atomically, growing pages or evicting older allocations when required.
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
        let strides = ElementStrides::of::<V, I, TM, FM>();
        let (vertices, records) = (
            geometry.buffer.vertices.len() as u64,
            feature_metadata.len() as u64,
        );
        if records > vertices {
            return Err(AllocationError::FeatureMetadata { vertices, records });
        }
        let sizes = [
            vertices * strides.vertex,
            geometry.buffer.indices.len() as u64 * strides.index,
            strides.layer_metadata,
            records * strides.feature_metadata,
        ];
        self.validate_sizes(sizes, strides)?;

        let alloc = self.reserve_page_allocation(sizes, strides, &mut protect)?;
        let first_vertex = alloc
            .vertices
            .start
            .checked_div(strides.vertex)
            .unwrap_or(0);
        let first_index = alloc.indices.start.checked_div(strides.index).unwrap_or(0);
        let (first_vertex_u32, first_index_u32) =
            match (element_index(first_vertex), element_index(first_index)) {
                (Ok(fv), Ok(fi)) => (fv, fi),
                (Err(err), _) | (_, Err(err)) => {
                    if let Some(page) = self
                        .pages
                        .get_mut(usize::from(alloc.page))
                        .and_then(Option::as_mut)
                    {
                        page.free(alloc.vertices, alloc.indices, alloc.layer_metadata);
                    }
                    return Err(err);
                }
            };

        let allocation_id = self.revision.wrapping_add(1);
        let entry = IndexEntry {
            allocation_id,
            paint_revision: 0,
            page: alloc.page,
            coords,
            style_layer,
            usable_indices: geometry.usable_indices,
            first_vertex: first_vertex_u32,
            first_index: first_index_u32,
            buffer_vertices: alloc.vertices,
            buffer_indices: alloc.indices,
            buffer_layer_metadata: alloc.layer_metadata,
            buffer_feature_metadata: first_vertex * strides.feature_metadata
                ..first_vertex * strides.feature_metadata + sizes[3],
        };

        let page = self.page_of(&entry);
        if sizes[0] > 0 {
            queue.write_buffer(
                &page.vertices.inner,
                entry.buffer_vertices.start,
                bytemuck::cast_slice(&geometry.buffer.vertices),
            );
        }
        if sizes[1] > 0 {
            queue.write_buffer(
                &page.indices.inner,
                entry.buffer_indices.start,
                bytemuck::cast_slice(&geometry.buffer.indices),
            );
        }
        if sizes[2] > 0 {
            queue.write_buffer(
                &page.layer_metadata.inner,
                entry.buffer_layer_metadata.start,
                bytemuck::bytes_of(&layer_metadata),
            );
        }
        if sizes[3] > 0 {
            queue.write_buffer(
                &page.feature_metadata.inner,
                entry.buffer_feature_metadata.start,
                bytemuck::cast_slice(feature_metadata),
            );
        }

        self.index.push_back(entry);
        self.revision = allocation_id;
        Ok(())
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
        let id = style_layer.id.clone();
        self.allocate_layer_geometry_unless(
            queue,
            coords,
            style_layer,
            geometry,
            layer_metadata,
            feature_metadata,
            protect,
        )?;
        let removed = self.index.remove_layer(coords, &id, Some(self.revision));
        for entry in &removed {
            self.free_entry_ranges(entry);
        }
        if !removed.is_empty() {
            self.reclaim_empty_pages(1);
        }
        Ok(())
    }

    pub(super) fn free_entry_ranges(&mut self, entry: &IndexEntry) {
        if let Some(page) = self
            .pages
            .get_mut(usize::from(entry.page))
            .and_then(Option::as_mut)
        {
            page.free(
                entry.buffer_vertices.clone(),
                entry.buffer_indices.clone(),
                entry.buffer_layer_metadata.clone(),
            );
        }
    }

    pub(super) fn allocated_capacities(&self) -> ByteCapacities {
        self.pages
            .iter()
            .flatten()
            .fold(ByteCapacities::default(), |acc, page| {
                acc.saturating_add(page.capacities())
            })
    }

    fn reserve_page_allocation(
        &mut self,
        sizes: [u64; 4],
        strides: ElementStrides,
        mut protect: impl FnMut(WorldTileCoords) -> bool,
    ) -> Result<PageAllocation, AllocationError> {
        // 1. Suballocate inside an already-allocated page if one has room.
        for (idx, slot) in self.pages.iter_mut().enumerate() {
            if let Some(page) = slot.as_mut() {
                if let Some(alloc) = page.try_allocate(idx as u16, sizes, strides) {
                    return Ok(alloc);
                }
            }
        }

        // 2. Grow by allocating a new page if total pool capacity permits.
        if let Some(alloc) = self.try_grow_page_and_alloc(sizes, strides) {
            return Ok(alloc);
        }

        // 3. Evict oldest unprotected allocations in FIFO order, coalescing freed blocks.
        while let Some(oldest) = self.index.front() {
            if protect(oldest.coords) {
                break;
            }
            let Some(evicted) = self.index.pop_front() else {
                break;
            };
            self.pattern_runs
                .remove(&(evicted.coords, evicted.style_layer.id.clone()));
            self.sort_runs
                .remove(&(evicted.coords, evicted.style_layer.id.clone()));
            let page_id = evicted.page;
            self.free_entry_ranges(&evicted);

            if let Some(page) = self
                .pages
                .get_mut(usize::from(page_id))
                .and_then(Option::as_mut)
            {
                if let Some(alloc) = page.try_allocate(page_id, sizes, strides) {
                    return Ok(alloc);
                }
                if page.is_empty() {
                    if let Some(alloc) = self.try_grow_page_and_alloc(sizes, strides) {
                        return Ok(alloc);
                    }
                }
            }
        }

        Err(self.saturation_error(sizes, strides))
    }
}

/// An element index as the 32 bits a draw call takes; a page beyond them is not drawable.
fn element_index(index: u64) -> Result<u32, AllocationError> {
    u32::try_from(index).map_err(|_| AllocationError::Capacity {
        buffer: BackingBufferType::Vertices,
        requested: index,
        capacity: u64::from(u32::MAX),
    })
}
